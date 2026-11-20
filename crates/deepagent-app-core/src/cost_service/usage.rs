//! USD estimates over the existing provider usage ledger. The configured CNY
//! budget remains a separate enforcement policy, never converted or relabeled.
use std::collections::{BTreeMap, HashMap};

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::{
    cost_store::{CostStore, UsageLedgerRow},
    Database,
};
use jiff::{tz::TimeZone, Timestamp};
use serde::Serialize;

use super::CostService;

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_miss: u64,
    pub cache_write: u64,
    /// Sum of priced rows only; null means no available estimate.
    pub cost_usd: Option<f64>,
    pub unpriced_requests: u64,
}

impl UsageTotals {
    fn add(&mut self, row: &UsageLedgerRow, bucket: UsageBuckets) {
        self.input += row.input;
        self.output += row.output;
        self.cache_read += bucket.read;
        self.cache_miss += row.input - bucket.read;
        self.cache_write += bucket.write;
        if let Some(cost) = bucket.estimate(&row.model, row.output) {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
        } else {
            self.unpriced_requests += 1;
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    pub date: String,
    pub models: BTreeMap<String, UsageTotals>,
}

struct UsageBuckets {
    fresh: u64,
    read: u64,
    write: u64,
    write_hour: u64,
}
impl UsageBuckets {
    fn new(row: &UsageLedgerRow) -> Self {
        let read = row.cache_read.min(row.input);
        let write = row.cache_write.min(row.input - read);
        Self {
            fresh: row.input - read - write,
            read,
            write,
            write_hour: row.cache_write_hour.min(write),
        }
    }
    fn estimate(&self, model: &str, output: u64) -> Option<f64> {
        let [input_price, output_price, read_price, write_price] = prices(model)?;
        // CC Switch treats a 1h Anthropic write as 1.6 times a 5m write.
        let write_price = if write_price == 0.0 {
            input_price
        } else {
            write_price
        };
        Some(
            (self.fresh as f64 * input_price
                + output as f64 * output_price
                + self.read as f64 * read_price
                + (self.write - self.write_hour) as f64 * write_price
                + self.write_hour as f64 * write_price * 1.6)
                / 1_000_000.0,
        )
    }
}

pub(crate) fn estimate_usage_usd(
    model: &str,
    usage: &deepagent_runtime::agent::RunUsage,
) -> Option<f64> {
    let read = usage
        .billed_cache_hit_tokens
        .min(usage.billed_prompt_tokens);
    UsageBuckets {
        fresh: (usage.billed_prompt_tokens - read).into(),
        read: read.into(),
        write: 0,
        write_hour: 0,
    }
    .estimate(model, usage.completion_tokens.into())
}

// USD per million: input, output, cached input, cache write. Standard tier
// estimates from the local CC Switch pricing catalog. Match an exact ID (or
// an explicitly priced dated variant); a gateway's actual charge can differ.
fn prices(model: &str) -> Option<[f64; 4]> {
    let name = model.to_ascii_lowercase();
    let name = name.rsplit('/').next()?;
    let exact = match name {
        "deepseek-flash" | "deepseek-v4-flash" | "deepseek-v4.1-flash" => [0.3, 1.2, 0.006, 0.0],
        "deepseek-chat" | "deepseek-reasoner" => [0.44, 1.32, 0.014, 0.0],
        "deepseek-v4-pro" => [1.32, 3.96, 0.044, 0.0],
        "gpt-6-astra" => [10.0, 50.0, 1.0, 12.5],
        "gpt-6-sol" => [2.0, 10.0, 0.20, 2.50],
        "gpt-6.1-sol" => [2.0, 10.0, 0.10, 2.50],
        "gpt-6.1-terra" => [1.0, 5.0, 0.05, 1.25],
        "gpt-6-luna" => [0.10, 0.50, 0.01, 0.125],
        "gpt-5.6-sol" | "gpt-5.6" => [4.0, 20.0, 0.40, 5.0],
        "gpt-5.6-terra" => [2.0, 12.0, 0.20, 2.50],
        "gpt-5.4-mini" => [0.75, 4.50, 0.075, 0.0],
        "claude-opus-5-5" => [4.0, 20.0, 0.20, 5.0],
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
            [5.0, 25.0, 0.50, 6.25]
        }
        "claude-sonnet-5-5" => [2.0, 10.0, 0.10, 2.50],
        "claude-sonnet-5" => [2.0, 10.0, 0.20, 2.50],
        "claude-haiku-5-5" => [0.10, 0.50, 0.01, 0.125],
        "kimi-k2.6" => [0.95, 4.0, 0.16, 0.0],
        "glm-5" => [1.0, 3.2, 0.2, 0.0],
        "qwen3.5-plus" => [0.26, 1.56, 0.052, 0.0],
        "grok-4.3" => [1.25, 2.50, 0.20, 0.0],
        _ => return None,
    };
    Some(exact)
}

pub(crate) struct SessionUsageEstimates {
    costs: HashMap<i64, Option<f64>>,
    pub(crate) totals: UsageTotals,
}

impl SessionUsageEstimates {
    pub(crate) fn load(db: &Database, session: &str) -> Result<Self> {
        Self::load_rows(CostStore::new(db).usage_rows(Some(session), i64::MIN, i64::MAX, None)?)
    }

    pub(crate) fn load_turn(db: &Database, session: &str, ids: &[i64]) -> Result<Self> {
        Self::load_rows(CostStore::new(db).usage_rows(
            Some(session),
            i64::MIN,
            i64::MAX,
            Some(ids),
        )?)
    }

    fn load_rows(rows: Vec<UsageLedgerRow>) -> Result<Self> {
        let mut costs = HashMap::new();
        let mut totals = UsageTotals::default();
        for row in rows {
            let bucket = UsageBuckets::new(&row);
            costs.insert(row.id, bucket.estimate(&row.model, row.output));
            totals.add(&row, bucket);
        }
        Ok(Self { costs, totals })
    }

    pub(crate) fn turn(&self, ids: &[i64]) -> Option<f64> {
        if ids.is_empty() {
            return None;
        }
        ids.iter()
            .try_fold(0.0, |sum, id| self.costs.get(id)?.map(|cost| sum + cost))
    }
}

impl CostService {
    pub fn session_usage_totals(&self, session: &str) -> Result<UsageTotals> {
        Ok(SessionUsageEstimates::load(&self.db, session)?.totals)
    }

    pub fn daily_usage(&self, start: i64, end: i64, timezone: &str) -> Result<Vec<DailyUsage>> {
        if start >= end || end.saturating_sub(start) > 800 * 86_400_000 {
            return Err(CoreError::invalid("invalid usage date range"));
        }
        let zone = TimeZone::get(timezone)
            .map_err(|e| CoreError::invalid(format!("usage timezone: {e}")))?;
        let mut days: BTreeMap<String, BTreeMap<String, UsageTotals>> = BTreeMap::new();
        for row in CostStore::new(&self.db).usage_rows(None, start, end, None)? {
            let date = Timestamp::from_millisecond(row.timestamp)
                .map_err(|e| CoreError::invalid(format!("usage timestamp: {e}")))?
                .to_zoned(zone.clone())
                .date()
                .to_string();
            days.entry(date)
                .or_default()
                .entry(row.model.clone())
                .or_default()
                .add(&row, UsageBuckets::new(&row));
        }
        Ok(days
            .into_iter()
            .map(|(date, models)| DailyUsage { date, models })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn daily_projection_books_tool_continuations_on_the_turn_day_once() {
        use deepagent_core::{
            clock::{Clock, SystemClock},
            id::SessionId,
        };
        use deepagent_persistence::{
            cost_store::{CostEntry, RunCallUsage},
            event_store::EventStore,
            run_store::RunStore,
        };
        let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        EventStore::new(&db)
            .create_session(session, None, SystemClock.now())
            .unwrap();
        let session = session.to_string();
        let millis = |date: &str| date.parse::<Timestamp>().unwrap().as_millisecond();
        let began = millis("2026-11-20T15:59:00Z"); // 23:59 in Shanghai
        let followup = millis("2026-11-20T16:01:00Z");
        RunStore::new(&db)
            .create("turn", &session, None, began)
            .unwrap();
        let store = CostStore::new(&db);
        let mut ids = Vec::new();
        for (id, timestamp, input, output, hit, write) in [
            ("first", began, 100, 10, 50, 10),
            ("tool-continuation", followup, 200, 20, 160, 20),
        ] {
            let report = serde_json::json!({"raw_usage":{"cache_creation_input_tokens":write}});
            let entry = CostEntry {
                session_id: &session,
                timestamp,
                model: "claude-sonnet-5",
                input_tokens: input,
                output_tokens: output,
                cache_hit_tokens: hit,
                cache_miss_tokens: input - hit,
                total_tokens: input + output,
                cost_yuan: None,
                input_cache_hit_price: None,
                input_cache_miss_price: None,
                output_price: None,
                pricing_source: None,
            };
            let call = RunCallUsage {
                session_id: &session,
                run_id: "turn",
                call_id: id,
                timestamp,
                report: &report,
            };
            ids.push(
                store
                    .record_run_call(&call, Some(&entry))
                    .unwrap()
                    .ledger_id
                    .unwrap(),
            );
            assert!(!store.record_run_call(&call, Some(&entry)).unwrap().inserted);
        }
        let estimated = SessionUsageEstimates::load_turn(&db, &session, &ids)
            .unwrap()
            .turn(&ids)
            .unwrap();
        let service = CostService::new(db);
        let days = service
            .daily_usage(began, followup + 1, "Asia/Shanghai")
            .unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].date, "2026-11-20");
        let model = &days[0].models["claude-sonnet-5"];
        assert_eq!(
            (
                model.input,
                model.output,
                model.cache_read,
                model.cache_miss,
                model.cache_write
            ),
            (300, 30, 210, 90, 30)
        );
        assert!((model.cost_usd.unwrap() - estimated).abs() < 1e-12);
        assert_eq!(service.session_usage_totals(&session).unwrap().input, 300);
    }

    #[test]
    fn cache_writes_are_not_charged_twice_and_unpriced_is_not_zero() {
        let row = UsageLedgerRow {
            id: 1,
            timestamp: 0,
            model: "claude-sonnet-5".into(),
            input: 1000,
            output: 100,
            cache_read: 500,
            cache_write: 200,
            cache_write_hour: 50,
        };
        let mut totals = UsageTotals::default();
        totals.add(&row, UsageBuckets::new(&row));
        assert_eq!(
            (
                totals.input,
                totals.output,
                totals.cache_read,
                totals.cache_miss,
                totals.cache_write
            ),
            (1000, 100, 500, 500, 200)
        );
        assert!((totals.cost_usd.unwrap() - 0.002275).abs() < 1e-12);
        let unknown = UsageLedgerRow {
            model: "private-model".into(),
            ..row
        };
        totals.add(&unknown, UsageBuckets::new(&unknown));
        assert_eq!((totals.input, totals.unpriced_requests), (2000, 1));
        assert!((totals.cost_usd.unwrap() - 0.002275).abs() < 1e-12);
    }
}
