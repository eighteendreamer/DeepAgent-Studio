//! Cost tracking and budget enforcement.
//!
//! Records per-request token cost in RMB/CNY, persists it to the `costs` table,
//! and enforces optional daily/monthly budget limits. Token counts come from the
//! provider's `usage` payload; pricing is centralized here so the UI only
//! displays authoritative backend-calculated values.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::cost_store::{CostEntry, CostStore};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

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
}

/// Cost tracking service.
pub struct CostService {
    db: Arc<Database>,
    pricing: RwLock<HashMap<String, ModelPricing>>,
    budget: RwLock<BudgetConfig>,
}

impl CostService {
    /// Build with the shared database, fallback RMB pricing, and no budget.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            pricing: RwLock::new(default_pricing()),
            budget: RwLock::new(BudgetConfig::default()),
        }
    }

    /// Replace the in-memory pricing catalog. Intended for a future official
    /// pricing refresh path; calculation callers keep using this service.
    pub fn set_pricing(&self, pricing: HashMap<String, ModelPricing>) {
        *self.pricing.write().unwrap_or_else(|p| p.into_inner()) = pricing;
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
    /// RMB cost.
    pub fn record(
        &self,
        session_id: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_hit_tokens: u32,
        cache_miss_tokens: u32,
        total_tokens: u32,
    ) -> Result<f64> {
        let pricing = self.pricing_for_model(model)?;
        let effective_cache_miss = if cache_miss_tokens > 0 {
            cache_miss_tokens
        } else {
            input_tokens.saturating_sub(cache_hit_tokens)
        };
        let cost = pricing.calculate(
            input_tokens,
            output_tokens,
            cache_hit_tokens,
            effective_cache_miss,
        );
        let now = SystemClock.now().as_millis();

        CostStore::new(&self.db).insert(&CostEntry {
            session_id,
            timestamp: now,
            model,
            input_tokens,
            output_tokens,
            cache_hit_tokens,
            cache_miss_tokens: effective_cache_miss,
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
        let month_start = now - (now % (day_ms * 30));

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
            currency: "CNY".to_string(),
            budget: self.budget(),
        })
    }

    fn pricing_for_model(&self, model: &str) -> Result<ModelPricing> {
        let pricing = self.pricing.read().unwrap_or_else(|p| p.into_inner());
        if let Some(p) = pricing.get(model) {
            return Ok(p.clone());
        }
        let lower = model.to_ascii_lowercase();
        if lower.contains("v4-flash") || lower.ends_with("-flash") {
            return pricing
                .get("deepseek-v4-flash")
                .cloned()
                .ok_or_else(|| CoreError::invalid("deepseek-v4-flash pricing is missing"));
        }
        if lower.contains("v4-pro") || lower.ends_with("-pro") {
            return pricing
                .get("deepseek-v4-pro")
                .cloned()
                .ok_or_else(|| CoreError::invalid("deepseek-v4-pro pricing is missing"));
        }
        if lower.contains("reasoner") {
            return pricing
                .get("deepseek-reasoner")
                .cloned()
                .ok_or_else(|| CoreError::invalid("deepseek-reasoner pricing is missing"));
        }
        Err(CoreError::invalid(format!(
            "no DeepSeek pricing configured for model '{model}'"
        )))
    }

    /// Check whether the budget allows a new run. Returns `Ok(())` if within
    /// budget, or an error describing which limit was hit.
    pub fn check_budget(&self) -> Result<()> {
        let budget = self.budget();
        if budget.daily_limit.is_none() && budget.monthly_limit.is_none() {
            return Ok(());
        }
        let summary = self.summary("")?;
        if let Some(daily) = budget.daily_limit {
            if summary.today_cost >= daily {
                return Err(CoreError::invalid(format!(
                    "daily budget exhausted (￥{:.2} / ￥{:.2})",
                    summary.today_cost, daily
                )));
            }
        }
        if let Some(monthly) = budget.monthly_limit {
            if summary.month_cost >= monthly {
                return Err(CoreError::invalid(format!(
                    "monthly budget exhausted (￥{:.2} / ￥{:.2})",
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
        let cost = svc
            .record("ses_1", "deepseek-v4-flash", 10000, 500, 8000, 2000, 10500)
            .unwrap();
        assert!(cost > 0.0);

        let summary = svc.summary("ses_1").unwrap();
        assert_eq!(summary.session_cost, cost);
        assert!(summary.total_cost >= cost);
        assert_eq!(summary.currency, "CNY");
    }

    #[test]
    fn budget_enforcement() {
        let svc = service();
        svc.set_budget(BudgetConfig {
            daily_limit: Some(0.001),
            monthly_limit: None,
        });
        svc.record(
            "ses_1",
            "deepseek-v4-flash",
            1_000_000,
            500_000,
            0,
            1_000_000,
            1_500_000,
        )
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
        let cost = svc
            .record("ses_1", "deepseek-chat", 1000, 1000, 0, 1000, 2000)
            .unwrap();
        assert!(cost > 0.0);
    }
}
