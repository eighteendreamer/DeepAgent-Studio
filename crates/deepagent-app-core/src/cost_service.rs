//! Cost tracking and budget enforcement (Phase 1B — gap-closure spec).
//!
//! Records per-request token cost (calculated from model pricing), persists it
//! to the `costs` table, and enforces optional daily/monthly budget limits.
//! The UI shows per-turn and cumulative spend; the runtime refuses new runs
//! when the budget is exhausted.

use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::cost_store::{CostEntry, CostStore};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

/// Pricing for one model (per million tokens, in ¥).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Cost per million input (prompt) tokens.
    pub input_per_million: f64,
    /// Cost per million output (completion) tokens.
    pub output_per_million: f64,
    /// Cost per million cache-hit tokens (discounted input).
    pub cache_hit_per_million: f64,
}

impl ModelPricing {
    /// Calculate the cost for a single request.
    pub fn calculate(&self, input_tokens: u32, output_tokens: u32, cache_hit_tokens: u32) -> f64 {
        // Cache-hit tokens are a subset of input tokens charged at the
        // discounted rate; the remainder is charged at the full input rate.
        let full_input = input_tokens.saturating_sub(cache_hit_tokens) as f64;
        let cost = (full_input * self.input_per_million
            + output_tokens as f64 * self.output_per_million
            + cache_hit_tokens as f64 * self.cache_hit_per_million)
            / 1_000_000.0;
        (cost * 10000.0).round() / 10000.0 // round to 4 decimal places
    }
}

/// Default DeepSeek pricing (as of 2025-06, ¥/百万 token).
pub fn default_pricing() -> std::collections::HashMap<String, ModelPricing> {
    let mut m = std::collections::HashMap::new();
    // deepseek-chat (V3): input ¥1/M, output ¥2/M, cache hit ¥0.1/M
    m.insert(
        "deepseek-chat".into(),
        ModelPricing {
            input_per_million: 1.0,
            output_per_million: 2.0,
            cache_hit_per_million: 0.1,
        },
    );
    // deepseek-reasoner (R1): input ¥4/M, output ¥16/M, cache hit ¥1/M
    m.insert(
        "deepseek-reasoner".into(),
        ModelPricing {
            input_per_million: 4.0,
            output_per_million: 16.0,
            cache_hit_per_million: 1.0,
        },
    );
    m
}

/// Budget configuration (optional limits).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Maximum daily spend in ¥ (None = unlimited).
    #[serde(default)]
    pub daily_limit: Option<f64>,
    /// Maximum monthly spend in ¥ (None = unlimited).
    #[serde(default)]
    pub monthly_limit: Option<f64>,
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
    pub total_tokens: u32,
    pub cost_yuan: f64,
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
    /// Budget config (for display).
    pub budget: BudgetConfig,
}

/// Cost tracking service.
pub struct CostService {
    db: Arc<Database>,
    pricing: std::collections::HashMap<String, ModelPricing>,
    budget: std::sync::RwLock<BudgetConfig>,
}

impl CostService {
    /// Build with the shared database, default pricing, and no budget.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            pricing: default_pricing(),
            budget: std::sync::RwLock::new(BudgetConfig::default()),
        }
    }

    /// Set the budget config. Uses interior mutability so the service can be
    /// shared as an `Arc` (the Tauri `set_budget` command calls this).
    pub fn set_budget(&self, budget: BudgetConfig) {
        *self.budget.write().unwrap_or_else(|p| p.into_inner()) = budget;
    }

    /// A clone of the current budget config.
    pub fn budget(&self) -> BudgetConfig {
        self.budget
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Record a cost entry for a completed model call. Returns the calculated
    /// cost in ¥.
    pub fn record(
        &self,
        session_id: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_hit_tokens: u32,
        total_tokens: u32,
    ) -> Result<f64> {
        let pricing = self
            .pricing
            .get(model)
            .or_else(|| self.pricing.get("deepseek-chat"))
            .cloned()
            .unwrap_or(ModelPricing {
                input_per_million: 1.0,
                output_per_million: 2.0,
                cache_hit_per_million: 0.1,
            });
        let cost = pricing.calculate(input_tokens, output_tokens, cache_hit_tokens);
        let now = SystemClock.now().as_millis();

        CostStore::new(&self.db).insert(&CostEntry {
            session_id,
            timestamp: now,
            model,
            input_tokens,
            output_tokens,
            cache_hit_tokens,
            total_tokens,
            cost_yuan: cost,
        })?;
        Ok(cost)
    }

    /// Get a cost summary for a session + today + month + total.
    pub fn summary(&self, session_id: &str) -> Result<CostSummary> {
        let now = SystemClock.now().as_millis();
        // Approximate day/month boundaries in ms.
        let day_ms = 86_400_000i64;
        let today_start = now - (now % day_ms);
        let month_start = now - (now % (day_ms * 30)); // approximate

        let store = CostStore::new(&self.db);
        let session_cost = store.session_total(session_id)?;
        let today_cost = store.total_since(today_start)?;
        let month_cost = store.total_since(month_start)?;
        let total_cost = store.total()?;

        Ok(CostSummary {
            session_cost,
            today_cost,
            month_cost,
            total_cost,
            budget: self.budget(),
        })
    }

    /// Check whether the budget allows a new run. Returns `Ok(())` if within
    /// budget, or an error describing which limit was hit.
    pub fn check_budget(&self) -> Result<()> {
        let budget = self.budget();
        if budget.daily_limit.is_none() && budget.monthly_limit.is_none() {
            return Ok(());
        }
        // Use a dummy session_id to get today/month totals.
        let summary = self.summary("")?;
        if let Some(daily) = budget.daily_limit {
            if summary.today_cost >= daily {
                return Err(CoreError::invalid(format!(
                    "daily budget exhausted (¥{:.2} / ¥{:.2})",
                    summary.today_cost, daily
                )));
            }
        }
        if let Some(monthly) = budget.monthly_limit {
            if summary.month_cost >= monthly {
                return Err(CoreError::invalid(format!(
                    "monthly budget exhausted (¥{:.2} / ¥{:.2})",
                    summary.month_cost, monthly
                )));
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for CostService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CostService")
            .field("budget", &self.budget())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> CostService {
        let db = Arc::new(Database::open_in_memory().unwrap());
        // The costs table has a FK to sessions(id); create the rows the tests use.
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

    #[test]
    fn pricing_calculation() {
        let p = ModelPricing {
            input_per_million: 1.0,
            output_per_million: 2.0,
            cache_hit_per_million: 0.1,
        };
        // 5000 input (3000 cached + 2000 full), 1000 output
        let cost = p.calculate(5000, 1000, 3000);
        // full_input = 2000, cost = (2000*1 + 1000*2 + 3000*0.1) / 1M = 4300/1M = 0.0043
        assert!((cost - 0.0043).abs() < 0.0001);
    }

    #[test]
    fn record_and_summary() {
        let svc = service();
        let cost = svc
            .record("ses_1", "deepseek-chat", 10000, 500, 8000, 10500)
            .unwrap();
        assert!(cost > 0.0);

        let summary = svc.summary("ses_1").unwrap();
        assert_eq!(summary.session_cost, cost);
        assert!(summary.total_cost >= cost);
    }

    #[test]
    fn budget_enforcement() {
        let svc = service();
        svc.set_budget(BudgetConfig {
            daily_limit: Some(0.001),
            monthly_limit: None,
        });
        // Record a cost that exceeds the tiny budget.
        svc.record("ses_1", "deepseek-chat", 1_000_000, 500_000, 0, 1_500_000)
            .unwrap();
        // Now check_budget should fail.
        assert!(svc.check_budget().is_err());
    }

    #[test]
    fn no_budget_always_passes() {
        let svc = service();
        assert!(svc.check_budget().is_ok());
    }
}
