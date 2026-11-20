//! Cost tracking and budget enforcement.
//!
//! Records per-request token cost in RMB/CNY, persists it to the `costs` table,
//! and enforces optional daily/monthly budget limits. Token counts come from the
//! provider's `usage` payload; pricing is centralized here so the UI only
//! displays authoritative backend-calculated values.

mod usage;
pub(crate) use usage::{estimate_usage_usd, SessionUsageEstimates};
pub use usage::{DailyUsage, UsageTotals};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::cost_store::{CostEntry, CostStore};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use jiff::{tz::TimeZone, Timestamp, Zoned};
use serde::{Deserialize, Serialize};

use crate::settings::SettingsService;

/// Pricing for one model, in RMB per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Cost per million cache-hit prompt tokens.
    pub input_cache_hit_per_million: f64,
    /// Cost per million cache-miss prompt tokens.
    pub input_cache_miss_per_million: f64,
    /// Cost per million output/completion tokens.
    pub output_per_million: f64,
}

impl ModelPricing {
    /// Calculate the RMB cost for a single request.
    pub fn calculate(
        &self,
        input_tokens: u32,
        output_tokens: u32,
        cache_hit_tokens: u32,
        cache_miss_tokens: u32,
    ) -> f64 {
        let miss_tokens = if cache_miss_tokens > 0 {
            cache_miss_tokens
        } else {
            input_tokens.saturating_sub(cache_hit_tokens)
        };
        let cost = (cache_hit_tokens as f64 * self.input_cache_hit_per_million
            + miss_tokens as f64 * self.input_cache_miss_per_million
            + output_tokens as f64 * self.output_per_million)
            / 1_000_000.0;
        (cost * 1_000_000.0).round() / 1_000_000.0
    }
}

/// Emergency fallback DeepSeek pricing snapshot (RMB / 1M tokens).
///
/// This snapshot is used when dynamic official pricing refresh is unavailable.
/// The UI never reads these constants directly.
pub fn default_pricing() -> HashMap<String, ModelPricing> {
    let mut m = HashMap::new();
    m.insert(
        "deepseek-v4-flash".into(),
        ModelPricing {
            input_cache_hit_per_million: 0.02,
            input_cache_miss_per_million: 1.0,
            output_per_million: 2.0,
        },
    );
    m.insert(
        "deepseek-v4-pro".into(),
        ModelPricing {
            input_cache_hit_per_million: 0.025,
            input_cache_miss_per_million: 3.0,
            output_per_million: 6.0,
        },
    );
    m.insert(
        "deepseek-reasoner".into(),
        ModelPricing {
            input_cache_hit_per_million: 0.02,
            input_cache_miss_per_million: 4.0,
            output_per_million: 16.0,
        },
    );
    // Deprecated aliases are mapped to the currently compatible v4 roles.
    m.insert(
        "deepseek-chat".into(),
        ModelPricing {
            input_cache_hit_per_million: 0.02,
            input_cache_miss_per_million: 1.0,
            output_per_million: 2.0,
        },
    );
    m
}

/// Budget configuration (optional limits, RMB/CNY).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Maximum daily spend in RMB (None = unlimited).
    #[serde(default)]
    pub daily_limit: Option<f64>,
    /// Maximum monthly spend in RMB (None = unlimited).
    #[serde(default)]
    pub monthly_limit: Option<f64>,
}

// Local policy for this cost ledger, not a provider-account spending limit.
// Reuse the settings document store; never maintain a second in-memory owner.
const BUDGET_COLLECTION: &str = "settings";
const BUDGET_ID: &str = "cost-budget";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetPolicy {
    version: u32,
    currency: String,
    period: String,
    timezone: String,
    scope: String,
    // Missing is corruption; only explicit null means unlimited. Public DTO
    // defaults remain compatible, but must not weaken a stored policy.
    #[serde(deserialize_with = "required_budget_limit")]
    daily_limit: Option<f64>,
    #[serde(deserialize_with = "required_budget_limit")]
    monthly_limit: Option<f64>,
}

fn required_budget_limit<'de, D>(deserializer: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(deserializer)
}

impl BudgetPolicy {
    fn new(budget: BudgetConfig) -> Self {
        Self {
            version: 1,
            currency: "CNY".into(),
            period: "calendar-day/month".into(),
            timezone: "host-local".into(),
            scope: "cost-ledger".into(),
            daily_limit: budget.daily_limit,
            monthly_limit: budget.monthly_limit,
        }
    }

    fn into_budget(self) -> Result<BudgetConfig> {
        if self.version != 1
            || self.currency != "CNY"
            || self.period != "calendar-day/month"
            || self.timezone != "host-local"
            || self.scope != "cost-ledger"
        {
            return Err(CoreError::invalid("unsupported budget policy metadata"));
        }
        let budget = BudgetConfig {
            daily_limit: self.daily_limit,
            monthly_limit: self.monthly_limit,
        };
        validate_budget(&budget)?;
        Ok(budget)
    }
}

fn validate_budget(budget: &BudgetConfig) -> Result<()> {
    for (name, limit) in [
        ("daily", budget.daily_limit),
        ("monthly", budget.monthly_limit),
    ] {
        if limit.is_some_and(|value| !value.is_finite() || value < 0.0) {
            return Err(CoreError::invalid(format!(
                "budget policy {name} limit must be finite and non-negative"
            )));
        }
    }
    Ok(())
}

/// A single persisted cost record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostRecord {
    pub session_id: String,
    pub timestamp: i64,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_hit_tokens: u32,
    pub cache_miss_tokens: u32,
    pub total_tokens: u32,
    /// Cost in RMB/CNY, persisted in the historical `cost_yuan` column.
    pub cost_yuan: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostRecordRequest {
    pub session_id: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_hit_tokens: u32,
    pub cache_miss_tokens: u32,
    pub total_tokens: u32,
}

/// Summary of accumulated costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostSummary {
    /// Cost of the current session.
    pub session_cost: f64,
    /// Cost today (since midnight local time).
    pub today_cost: f64,
    /// Cost this month.
    pub month_cost: f64,
    /// All-time total cost.
    pub total_cost: f64,
    /// Currency used for all cost values.
    pub currency: String,
    /// Budget config (for display).
    pub budget: BudgetConfig,
    /// Cumulative input tokens for the current session.
    pub input_tokens: u64,
    /// Cumulative output tokens for the current session.
    pub output_tokens: u64,
    /// Cumulative cache-hit tokens for the current session.
    pub cache_hit_tokens: u64,
    /// Cumulative cache-miss tokens for the current session.
    pub cache_miss_tokens: u64,
}

/// Cost tracking service.
pub struct CostService {
    db: Arc<Database>,
    pricing: RwLock<PricingSource>,
}

enum PricingSource {
    Explicit(HashMap<String, ModelPricing>),
    Settings(Arc<SettingsService>),
}

impl CostService {
    /// Build a standalone ledger with no initial pricing. Use `set_pricing`
    /// for an explicit catalog, or `from_settings` for a settings-owned host.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            pricing: RwLock::new(PricingSource::Explicit(HashMap::new())),
        }
    }

    /// Bind to the host's settings owner instead of copying its pricing.
    /// Successful initialization/refresh is immediately visible to billing;
    /// no background notification or ordering-sensitive `set_pricing` is needed.
    pub fn from_settings(db: Arc<Database>, settings: Arc<SettingsService>) -> Self {
        Self {
            db,
            pricing: RwLock::new(PricingSource::Settings(settings)),
        }
    }

    /// Select an explicit catalog (also opts out of a settings binding).
    /// Settings-owned production hosts should not call this method.
    pub fn set_pricing(&self, pricing: HashMap<String, ModelPricing>) {
        *self.pricing.write().unwrap_or_else(|p| p.into_inner()) = PricingSource::Explicit(pricing);
    }

    /// Atomically replace the ledger's durable policy. Success means committed;
    /// concurrent replacements use last-committed-write wins, not a field merge.
    /// Explicit `None` clears a limit; zero blocks new runs immediately.
    pub fn set_budget(&self, budget: BudgetConfig) -> Result<()> {
        validate_budget(&budget)?;
        let body = serde_json::to_string(&BudgetPolicy::new(budget))
            .map_err(|error| CoreError::Serialization(format!("budget policy encode: {error}")))?;
        DocumentStore::new(&self.db)
            .put(BUDGET_COLLECTION, BUDGET_ID, &body, None, SystemClock.now())
            .map_err(|error| CoreError::Persistence(format!("budget policy write: {error}")))?;
        tracing::info!(scope = "cost-ledger", "budget policy committed");
        Ok(())
    }

    /// Read the shared policy on every operation, including across independent
    /// connections/processes. Only an absent document means never configured;
    /// unreadable, corrupt or unsupported policies must not become unlimited.
    pub fn budget(&self) -> Result<BudgetConfig> {
        let Some(document) = DocumentStore::new(&self.db)
            .get(BUDGET_COLLECTION, BUDGET_ID)
            .map_err(|error| CoreError::Persistence(format!("budget policy read: {error}")))?
        else {
            return Ok(BudgetConfig::default());
        };
        let policy: BudgetPolicy = serde_json::from_str(&document.body).map_err(|error| {
            // Parser messages can echo arbitrary stored strings. Preserve the
            // failure category/location without logging the policy body.
            CoreError::Serialization(format!(
                "budget policy decode: {:?} at line {}, column {}",
                error.classify(),
                error.line(),
                error.column()
            ))
        })?;
        policy.into_budget()
    }

    /// Record a cost entry for a completed model call. Returns the calculated
    /// RMB cost when pricing is available, or None when pricing is unavailable.
    /// Token usage is always saved; cost unavailability is marked explicitly.
    pub fn record(&self, request: CostRecordRequest) -> Result<Option<f64>> {
        self.record_with_id(request).map(|(_, cost)| cost)
    }

    pub(crate) fn record_with_id(&self, request: CostRecordRequest) -> Result<(i64, Option<f64>)> {
        self.with_priced_entry(&request, |entry| {
            let id = CostStore::new(&self.db).insert_with_id(entry)?;
            Ok((id, entry.cost_yuan))
        })
    }

    pub(crate) fn record_run_call(
        &self,
        call: &deepagent_persistence::cost_store::RunCallUsage<'_>,
        request: Option<CostRecordRequest>,
    ) -> Result<deepagent_persistence::cost_store::RecordedCallCost> {
        let store = CostStore::new(&self.db);
        match request {
            Some(request) => {
                self.with_priced_entry(&request, |entry| store.record_run_call(call, Some(entry)))
            }
            None => store.record_run_call(call, None),
        }
    }

    /// One pricing/cache normalization boundary for aggregate and owned calls.
    fn with_priced_entry<T>(
        &self,
        request: &CostRecordRequest,
        write: impl FnOnce(&CostEntry<'_>) -> Result<T>,
    ) -> Result<T> {
        let pricing_result = self.pricing_for_model(&request.model);
        let effective_cache_miss = if request.cache_miss_tokens > 0 {
            request.cache_miss_tokens
        } else {
            request
                .input_tokens
                .saturating_sub(request.cache_hit_tokens)
        };
        let now = SystemClock.now().as_millis();

        let (cost_yuan, snapshot) = match pricing_result {
            Ok(pricing) => {
                let cost = pricing.calculate(
                    request.input_tokens,
                    request.output_tokens,
                    request.cache_hit_tokens,
                    effective_cache_miss,
                );
                (
                    Some(cost),
                    Some((
                        pricing.input_cache_hit_per_million,
                        pricing.input_cache_miss_per_million,
                        pricing.output_per_million,
                    )),
                )
            }
            Err(error) => {
                tracing::warn!(
                    model = %request.model,
                    %error,
                    "no pricing available; saving token usage without cost"
                );
                (None, None)
            }
        };

        write(&CostEntry {
            session_id: &request.session_id,
            timestamp: now,
            model: &request.model,
            input_tokens: request.input_tokens,
            output_tokens: request.output_tokens,
            cache_hit_tokens: request.cache_hit_tokens,
            cache_miss_tokens: effective_cache_miss,
            total_tokens: request.total_tokens,
            cost_yuan,
            input_cache_hit_price: snapshot.map(|s| s.0),
            input_cache_miss_price: snapshot.map(|s| s.1),
            output_price: snapshot.map(|s| s.2),
            pricing_source: Some("https://api-docs.deepseek.com/zh-cn/quick_start/pricing"),
        })
    }

    /// Get a cost summary for a session + today + month + total.
    pub fn summary(&self, session_id: &str) -> Result<CostSummary> {
        self.summary_with_budget(session_id, self.budget()?)
    }

    fn summary_with_budget(&self, session_id: &str, budget: BudgetConfig) -> Result<CostSummary> {
        // Capture one instant and one full set of host-local zone rules. A
        // current fixed UTC offset is not necessarily the offset at month start.
        let now = Timestamp::from_millisecond(SystemClock.now().as_millis())
            .map_err(|error| CoreError::invalid(format!("cost calendar timestamp: {error}")))?;
        let zone = TimeZone::try_system().map_err(|error| {
            CoreError::invalid(format!("cost calendar system timezone: {error}"))
        })?;
        self.summary_at(session_id, &now.to_zoned(zone), budget)
    }

    fn summary_at(
        &self,
        session_id: &str,
        now: &Zoned,
        budget: BudgetConfig,
    ) -> Result<CostSummary> {
        // Date::to_zoned uses compatible disambiguation: first occurrence in
        // a fold, shifted forward across a midnight gap. No 24h/30d buckets.
        let start = |date: jiff::civil::Date| {
            date.to_zoned(now.time_zone().clone())
                .map(|start| start.timestamp().as_millisecond())
                .map_err(|error| CoreError::invalid(format!("cost calendar period start: {error}")))
        };
        let today_start = start(now.date())?;
        let month_start = start(now.date().first_of_month())?;

        let store = CostStore::new(&self.db);
        let session_cost = store.session_total(session_id)?;
        let today_cost = store.total_since(today_start)?;
        let month_cost = store.total_since(month_start)?;
        let total_cost = store.total()?;
        let usage = store.session_usage(session_id)?;

        Ok(CostSummary {
            session_cost,
            today_cost,
            month_cost,
            total_cost,
            currency: "CNY".to_string(),
            budget,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_hit_tokens: usage.cache_hit_tokens,
            cache_miss_tokens: usage.cache_miss_tokens,
        })
    }

    fn pricing_for_model(&self, model: &str) -> Result<ModelPricing> {
        let source = self.pricing.read().unwrap_or_else(|p| p.into_inner());
        let pricing = match &*source {
            PricingSource::Explicit(pricing) => pricing.get(model).cloned(),
            PricingSource::Settings(settings) => settings
                .load()?
                .and_then(|settings| settings.pricing_catalog.models.get(model).cloned()),
        };

        // Exact match first (e.g. "deepseek-flash", "deepseek-v4-pro")
        if let Some(pricing) = pricing {
            return Ok(pricing);
        }

        // No fuzzy fallback: if the exact model isn't in the official catalog,
        // billing cannot proceed. This enforces "pricing must come from official
        // dynamic data, never hardcoded guesses".
        Err(CoreError::invalid(format!(
            "no official pricing for model '{model}'; refresh pricing via settings"
        )))
    }

    /// Check admission against measured, committed ledger spend. This same
    /// policy is used before each owned physical request, not only new runs.
    pub fn check_budget(&self) -> Result<()> {
        self.check_budget_accounting(true)
    }

    pub(crate) fn check_budget_accounting(&self, accounting_complete: bool) -> Result<()> {
        match self.budget_denial(accounting_complete)? {
            Some(reason) => Err(CoreError::invalid(reason)),
            None => Ok(()),
        }
    }

    /// Optional work may skip exhaustion, never broken policy or storage.
    pub(crate) fn allows_optional_request(&self) -> Result<bool> {
        Ok(self.budget_denial(true)?.is_none())
    }

    fn budget_denial(&self, accounting_complete: bool) -> Result<Option<String>> {
        let budget = self.budget()?;
        if budget.daily_limit.is_none() && budget.monthly_limit.is_none() {
            return Ok(None);
        }
        if !accounting_complete {
            return Err(CoreError::invalid(
                "budget cannot be verified: prior model call usage or price is unavailable",
            ));
        }
        let summary = self.summary_with_budget("", budget)?;
        if let Some(daily) = summary.budget.daily_limit {
            if summary.today_cost >= daily {
                return Ok(Some(format!(
                    "daily budget exhausted (￥{:.2} / ￥{:.2})",
                    summary.today_cost, daily
                )));
            }
        }
        if let Some(monthly) = summary.budget.monthly_limit {
            if summary.month_cost >= monthly {
                return Ok(Some(format!(
                    "monthly budget exhausted (￥{:.2} / ￥{:.2})",
                    summary.month_cost, monthly
                )));
            }
        }
        Ok(None)
    }
}

impl std::fmt::Debug for CostService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Formatting must not perform database IO or reveal stored policy.
        f.debug_struct("CostService").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod budget_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> CostService {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO sessions (id, title, mode, created_at, updated_at) \
                 VALUES ('ses_1', 't', 'normal', 0, 0)",
                [],
            )
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
            c.execute(
                "INSERT INTO sessions (id, title, mode, created_at, updated_at) \
                 VALUES ('', 't', 'normal', 0, 0)",
                [],
            )
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
            Ok(())
        })
        .unwrap();
        CostService::new(db)
    }

    fn assert_calendar_cost(now: &str, day_start: &str, month_start: &str) {
        let svc = service();
        let local: Zoned = now.parse().unwrap();
        // Explicit UTC expectations are independent of the period calculation.
        let day: Timestamp = day_start.parse().unwrap();
        let month: Timestamp = month_start.parse().unwrap();
        let day = day.as_millisecond();
        let month = month.as_millisecond();
        assert!(month <= day && day <= local.timestamp().as_millisecond());
        for (timestamp, amount) in [
            (month - 1, 32.0),
            (month, 2.0),
            (day - 1, 4.0),
            (day, 8.0),
            (local.timestamp().as_millisecond(), 16.0),
        ] {
            CostStore::new(&svc.db)
                .insert(&CostEntry {
                    session_id: "ses_1",
                    timestamp,
                    model: "calendar-fixture",
                    input_tokens: 1,
                    output_tokens: 0,
                    cache_hit_tokens: 0,
                    cache_miss_tokens: 1,
                    total_tokens: 1,
                    cost_yuan: Some(amount),
                    input_cache_hit_price: None,
                    input_cache_miss_price: None,
                    output_price: None,
                    pricing_source: Some("synthetic-calendar-boundary"),
                })
                .unwrap();
        }
        let summary = svc
            .summary_at("ses_1", &local, svc.budget().unwrap())
            .unwrap();
        assert_eq!(summary.session_cost, 62.0, "{now}");
        assert_eq!(summary.total_cost, 62.0, "{now}");
        assert_eq!(
            summary.today_cost,
            if day == month { 26.0 } else { 24.0 },
            "{now}"
        );
        assert_eq!(
            summary.month_cost,
            if day == month { 26.0 } else { 30.0 },
            "{now}"
        );
        assert_eq!(summary.currency, "CNY");
        assert_eq!(summary.budget, BudgetConfig::default());
        assert_eq!(summary.input_tokens, 5);
        assert_eq!(summary.cache_miss_tokens, 5);
        assert_eq!(summary.output_tokens, 0);
        assert_eq!(summary.cache_hit_tokens, 0);
    }

    #[test]
    fn calendar_cost_uses_local_dates_and_offsets() {
        for (now, day, month) in [
            (
                "2026-10-30T12:00:00Z[UTC]",
                "2026-10-30T00:00:00Z",
                "2026-10-01T00:00:00Z",
            ),
            (
                "2026-10-30T00:00:00+08:00[Asia/Shanghai]",
                "2026-10-29T16:00:00Z",
                "2026-09-30T16:00:00Z",
            ),
            (
                "2026-10-29T23:59:59.999+08:00[Asia/Shanghai]",
                "2026-10-28T16:00:00Z",
                "2026-09-30T16:00:00Z",
            ),
            (
                "2026-10-29T23:59:59.999-07:00[-07:00]",
                "2026-10-29T07:00:00Z",
                "2026-10-01T07:00:00Z",
            ),
            (
                "2026-10-30T12:00:00+05:45[Asia/Kathmandu]",
                "2026-10-29T18:15:00Z",
                "2026-09-30T18:15:00Z",
            ),
            (
                "2026-10-30T12:00:00-09:30[-09:30]",
                "2026-10-30T09:30:00Z",
                "2026-10-01T09:30:00Z",
            ),
            (
                "1970-01-01T00:00:00Z[UTC]",
                "1970-01-01T00:00:00Z",
                "1970-01-01T00:00:00Z",
            ),
            (
                "1969-12-31T23:59:59Z[UTC]",
                "1969-12-31T00:00:00Z",
                "1969-12-01T00:00:00Z",
            ),
        ] {
            assert_calendar_cost(now, day, month);
        }
    }

    #[test]
    fn calendar_cost_handles_month_lengths_leap_days_and_year_rollover() {
        for (now, day, month) in [
            (
                "2023-02-28T12:00:00Z[UTC]",
                "2023-02-28T00:00:00Z",
                "2023-02-01T00:00:00Z",
            ),
            (
                "2024-02-29T12:00:00Z[UTC]",
                "2024-02-29T00:00:00Z",
                "2024-02-01T00:00:00Z",
            ),
            (
                "2024-03-01T00:00:00Z[UTC]",
                "2024-03-01T00:00:00Z",
                "2024-03-01T00:00:00Z",
            ),
            (
                "2026-04-30T23:59:59.999Z[UTC]",
                "2026-04-30T00:00:00Z",
                "2026-04-01T00:00:00Z",
            ),
            (
                "2026-05-31T12:00:00Z[UTC]",
                "2026-05-31T00:00:00Z",
                "2026-05-01T00:00:00Z",
            ),
            (
                "2026-12-31T23:59:59.999Z[UTC]",
                "2026-12-31T00:00:00Z",
                "2026-12-01T00:00:00Z",
            ),
            (
                "2027-01-01T00:00:00Z[UTC]",
                "2027-01-01T00:00:00Z",
                "2027-01-01T00:00:00Z",
            ),
        ] {
            assert_calendar_cost(now, day, month);
        }
    }

    #[test]
    fn calendar_cost_uses_boundary_offsets_across_dst() {
        for (now, day, month) in [
            (
                "2024-03-10T12:00:00-04:00[America/New_York]",
                "2024-03-10T05:00:00Z",
                "2024-03-01T05:00:00Z",
            ),
            (
                "2024-11-03T12:00:00-05:00[America/New_York]",
                "2024-11-03T04:00:00Z",
                "2024-11-01T04:00:00Z",
            ),
            (
                "2024-03-15T12:00:00-04:00[America/New_York]",
                "2024-03-15T04:00:00Z",
                "2024-03-01T05:00:00Z",
            ),
            (
                "2024-11-15T12:00:00-05:00[America/New_York]",
                "2024-11-15T05:00:00Z",
                "2024-11-01T04:00:00Z",
            ),
            (
                "2024-10-06T12:00:00+11:00[Australia/Lord_Howe]",
                "2024-10-05T13:30:00Z",
                "2024-09-30T13:30:00Z",
            ),
            (
                "2024-04-07T12:00:00+10:30[Australia/Lord_Howe]",
                "2024-04-06T13:00:00Z",
                "2024-03-31T13:00:00Z",
            ),
        ] {
            assert_calendar_cost(now, day, month);
        }
    }

    #[test]
    fn calendar_cost_disambiguates_midnight_gaps_folds_and_skipped_dates() {
        for (now, day, month) in [
            (
                "2015-10-18T12:00:00-02:00[America/Sao_Paulo]",
                "2015-10-18T03:00:00Z",
                "2015-10-01T03:00:00Z",
            ),
            (
                "2024-11-03T00:30:00-05:00[America/Havana]",
                "2024-11-03T04:00:00Z",
                "2024-11-01T04:00:00Z",
            ),
            (
                "2024-11-03T00:30:00-04:00[America/Havana]",
                "2024-11-03T04:00:00Z",
                "2024-11-01T04:00:00Z",
            ),
            (
                "2011-12-31T12:00:00+14:00[Pacific/Apia]",
                "2011-12-30T10:00:00Z",
                "2011-12-01T10:00:00Z",
            ),
        ] {
            assert_calendar_cost(now, day, month);
        }
    }

    #[test]
    fn pricing_calculation() {
        let p = ModelPricing {
            input_cache_hit_per_million: 0.02,
            input_cache_miss_per_million: 1.0,
            output_per_million: 2.0,
        };
        // 5000 input (3000 cached + 2000 miss), 1000 output.
        let cost = p.calculate(5000, 1000, 3000, 2000);
        assert!((cost - 0.00406).abs() < 0.000001);
    }

    #[test]
    fn record_and_summary() {
        let svc = service();
        svc.set_pricing(default_pricing());
        let cost = svc
            .record(CostRecordRequest {
                session_id: "ses_1".to_string(),
                model: "deepseek-v4-flash".to_string(),
                input_tokens: 10000,
                output_tokens: 500,
                cache_hit_tokens: 8000,
                cache_miss_tokens: 2000,
                total_tokens: 10500,
            })
            .unwrap();
        assert!(cost.is_some());
        let cost = cost.unwrap();
        assert!(cost > 0.0);

        let summary = svc.summary("ses_1").unwrap();
        assert_eq!(summary.session_cost, cost);
        assert!(summary.total_cost >= cost);
        assert_eq!(summary.currency, "CNY");
    }

    #[test]
    fn budget_enforcement() {
        let svc = service();
        svc.set_pricing(default_pricing());
        svc.set_budget(BudgetConfig {
            daily_limit: Some(0.001),
            monthly_limit: None,
        })
        .unwrap();
        svc.record(CostRecordRequest {
            session_id: "ses_1".to_string(),
            model: "deepseek-v4-flash".to_string(),
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_hit_tokens: 0,
            cache_miss_tokens: 1_000_000,
            total_tokens: 1_500_000,
        })
        .unwrap();
        assert!(svc.check_budget().is_err());
    }

    #[test]
    fn no_budget_always_passes() {
        let svc = service();
        assert!(svc.check_budget().is_ok());
    }

    #[test]
    fn deprecated_chat_alias_uses_flash_pricing() {
        let svc = service();
        svc.set_pricing(default_pricing());
        let cost = svc
            .record(CostRecordRequest {
                session_id: "ses_1".to_string(),
                model: "deepseek-chat".to_string(),
                input_tokens: 1000,
                output_tokens: 1000,
                cache_hit_tokens: 0,
                cache_miss_tokens: 1000,
                total_tokens: 2000,
            })
            .unwrap();
        assert!(cost.is_some());
        assert!(cost.unwrap() > 0.0);
    }

    #[test]
    fn no_pricing_saves_usage_without_cost() {
        let svc = service();
        // No pricing set
        let result = svc
            .record(CostRecordRequest {
                session_id: "ses_1".to_string(),
                model: "unknown-model".to_string(),
                input_tokens: 1000,
                output_tokens: 500,
                cache_hit_tokens: 0,
                cache_miss_tokens: 1000,
                total_tokens: 1500,
            })
            .unwrap();
        assert!(result.is_none());
    }
}
