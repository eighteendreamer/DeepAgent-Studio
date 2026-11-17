//! DeepSeek account balance query (`GET /user/balance`).
//!
//! Mirrors the official endpoint described at
//! <https://api-docs.deepseek.com/api/get-user-balance>. The response carries an
//! `is_available` flag plus a per-currency breakdown (granted credit + topped-up
//! credit + total). Transport-agnostic — every call goes through
//! [`HttpTransport::get_json`], so tests run offline via the mock transport.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use deepagent_core::error::{CoreError, Result};

use crate::transport::HttpTransport;

/// Endpoint path appended to the DeepSeek base URL.
pub const BALANCE_PATH: &str = "/user/balance";
const NEW_API_SUBSCRIPTION_PATH: &str = "dashboard/billing/subscription";
const NEW_API_USAGE_PATH: &str = "dashboard/billing/usage";
const NEW_API_TOKEN_USAGE_PATH: &str = "api/usage/token/";
const SUB2API_USAGE_PATH: &str = "usage";

/// Provider-specific balance endpoint selection stored in the harness registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceKind {
    None,
    Auto,
    Official,
    NewApi,
    Sub2Api,
}

impl BalanceKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "auto" => Ok(Self::Auto),
            "official" => Ok(Self::Official),
            "new_api" => Ok(Self::NewApi),
            "sub2api" => Ok(Self::Sub2Api),
            other => Err(CoreError::invalid(format!(
                "unsupported balance kind `{other}`"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Auto => "auto",
            Self::Official => "official",
            Self::NewApi => "new_api",
            Self::Sub2Api => "sub2api",
        }
    }
}

/// A normalized provider balance. `unlimited` is intentionally separate from
/// `amount`; unlimited plans must never be represented by a made-up maximum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceAmount {
    pub amount: Option<f64>,
    pub unit: String,
    pub unlimited: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BalanceSnapshot {
    pub amount: BalanceAmount,
    pub available: Option<bool>,
}

/// One per-currency balance row returned by DeepSeek. Amounts arrive as
/// decimal strings (e.g. `"100.00"`); we keep them as strings to avoid losing
/// precision and because the UI just renders them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BalanceInfo {
    /// Currency code, e.g. `"CNY"` / `"USD"`.
    #[serde(default)]
    pub currency: String,
    /// Total spendable balance (granted + topped-up).
    #[serde(default)]
    pub total_balance: String,
    /// Granted (free credit) portion.
    #[serde(default)]
    pub granted_balance: String,
    /// Topped-up (paid) portion.
    #[serde(default)]
    pub topped_up_balance: String,
}

/// Full response shape from `GET /user/balance`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BalanceResponse {
    /// Whether the account currently has any spendable balance.
    #[serde(default)]
    pub is_available: bool,
    /// Per-currency balance breakdown.
    #[serde(default)]
    pub balance_infos: Vec<BalanceInfo>,
}

/// Query the user's DeepSeek balance. `base_url` should be the same value
/// stored in the catalog (`https://api.deepseek.com`). Bearer-auth is supplied
/// by [`HttpTransport::get_json`] from the raw `api_key`.
pub async fn fetch_balance(
    transport: Arc<dyn HttpTransport>,
    base_url: &str,
    api_key: &str,
) -> Result<BalanceResponse> {
    if api_key.trim().is_empty() {
        return Err(CoreError::invalid("API key must not be empty"));
    }
    let url = format!("{}{}", base_url.trim_end_matches('/'), BALANCE_PATH);
    let body = transport.get_json(&url, api_key).await?;
    serde_json::from_str::<BalanceResponse>(&body)
        .map_err(|e| CoreError::other(format!("parse balance response: {e}")))
}

/// Query and normalize one harness provider's configured balance endpoint.
/// `Auto` tries New API first and then sub2api, preserving both failure
/// categories without ever echoing credentials or provider response bodies.
pub async fn fetch_balance_by_kind(
    transport: Arc<dyn HttpTransport>,
    base_url: &str,
    api_key: &str,
    kind: BalanceKind,
) -> Result<BalanceSnapshot> {
    if api_key.trim().is_empty() && kind != BalanceKind::None {
        return Err(CoreError::invalid("API key must not be empty"));
    }
    match kind {
        BalanceKind::None => Ok(BalanceSnapshot {
            amount: BalanceAmount {
                amount: None,
                unit: "none".into(),
                unlimited: false,
            },
            available: None,
        }),
        BalanceKind::Official => fetch_official_balance(transport, base_url, api_key).await,
        BalanceKind::NewApi => fetch_new_api_balance(transport, base_url, api_key).await,
        BalanceKind::Sub2Api => fetch_sub2api_balance(transport, base_url, api_key).await,
        BalanceKind::Auto => {
            match fetch_new_api_balance(transport.clone(), base_url, api_key).await {
                Ok(snapshot) => Ok(snapshot),
                Err(new_api) => match fetch_sub2api_balance(transport, base_url, api_key).await {
                    Ok(snapshot) => Ok(snapshot),
                    Err(sub2api) => Err(CoreError::other(format!(
                        "PROVIDER_USAGE_DETECT_FAILED: new_api: {}; sub2api: {}",
                        scrub_balance_error(&new_api, api_key),
                        scrub_balance_error(&sub2api, api_key),
                    ))),
                },
            }
        }
    }
}

async fn fetch_official_balance(
    transport: Arc<dyn HttpTransport>,
    base_url: &str,
    api_key: &str,
) -> Result<BalanceSnapshot> {
    let mut url = Url::parse(&endpoint(base_url, BALANCE_PATH)?)
        .map_err(|_| CoreError::invalid("provider base URL is invalid"))?;
    if url.host_str() == Some("api.deepseek.com") {
        url.set_path(BALANCE_PATH);
    }
    let body = transport.get_json(url.as_str(), api_key).await?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|error| CoreError::Serialization(format!("parse official balance: {error}")))?;
    let available = value
        .get("is_available")
        .and_then(Value::as_bool)
        .ok_or_else(|| CoreError::Serialization("official balance missing is_available".into()))?;
    let rows = value
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::Serialization("official balance missing balance_infos".into()))?;
    let row = rows
        .iter()
        .find(|item| item.get("currency").and_then(Value::as_str) == Some("CNY"))
        .or_else(|| rows.first());
    let Some(row) = row else {
        return Ok(BalanceSnapshot {
            amount: BalanceAmount {
                amount: None,
                unit: "CNY".into(),
                unlimited: false,
            },
            available: Some(available),
        });
    };
    let unit = row
        .get("currency")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CoreError::Serialization("official balance missing currency".into()))?;
    let amount = number(row.get("total_balance"))
        .ok_or_else(|| CoreError::Serialization("official balance missing total_balance".into()))?;
    Ok(BalanceSnapshot {
        amount: BalanceAmount {
            amount: Some(amount),
            unit: unit.to_owned(),
            unlimited: false,
        },
        available: Some(available),
    })
}

async fn fetch_new_api_balance(
    transport: Arc<dyn HttpTransport>,
    base_url: &str,
    api_key: &str,
) -> Result<BalanceSnapshot> {
    let subscription = get_value(
        transport.clone(),
        &endpoint(base_url, NEW_API_SUBSCRIPTION_PATH)?,
        api_key,
    )
    .await?;
    let usage = get_value(
        transport.clone(),
        &endpoint(base_url, NEW_API_USAGE_PATH)?,
        api_key,
    )
    .await?;
    let token_usage = get_value(
        transport,
        &endpoint_without_v1(base_url, NEW_API_TOKEN_USAGE_PATH)?,
        api_key,
    )
    .await
    .ok();

    // Billing's total_usage is in cents. Prefer a token endpoint's display
    // object as one coherent unit; never mix it with legacy billing amounts.
    let token_data = token_usage.as_ref().and_then(|value| value.get("data"));
    let limit = number_at(&subscription, &["hard_limit_usd"])
        .or_else(|| number_at(&subscription, &["soft_limit_usd"]))
        .or_else(|| number_at(&subscription, &["system_hard_limit_usd"]));
    let used = number_at(&usage, &["total_usage"]).map(|cents| cents / 100.0);
    let unlimited = token_data
        .and_then(|data| bool_at(data, &["unlimited_quota"]))
        .or_else(|| bool_at(&subscription, &["unlimited_quota"]))
        .unwrap_or_else(|| {
            ["hard_limit_usd", "soft_limit_usd", "system_hard_limit_usd"]
                .iter()
                .all(|field| number_at(&subscription, &[field]) == Some(100_000_000.0))
        });
    let display = token_data.and_then(|data| data.get("display"));
    let display_unit = display
        .and_then(|value| string_at(value, &["unit"]))
        .map(|unit| unit.trim().to_uppercase())
        .filter(|unit| !unit.is_empty());
    if !unlimited && limit.is_none() && used.is_none() && display_unit.is_none() {
        return Err(CoreError::Serialization(
            "new_api balance has no quota or usage".into(),
        ));
    }
    let remaining = if let Some(display) = display.filter(|_| display_unit.is_some()) {
        number_at(display, &["remaining"]).or_else(|| {
            subtract_amounts(
                number_at(display, &["total"]),
                number_at(display, &["used"]),
            )
        })
    } else if unlimited {
        None
    } else {
        subtract_amounts(limit, used)
    };
    let amount = remaining
        .and_then(amount_micros)
        .map(|value| value as f64 / 1_000_000.0);
    Ok(BalanceSnapshot {
        amount: BalanceAmount {
            amount,
            // Legacy *_usd fields are also used by relays reporting CNY or
            // tokens. Without display.unit, leave the currency unspecified.
            unit: display_unit.unwrap_or_default(),
            unlimited,
        },
        available: amount
            .map(|value| value > 0.0)
            .or(unlimited.then_some(true)),
    })
}

async fn fetch_sub2api_balance(
    transport: Arc<dyn HttpTransport>,
    base_url: &str,
    api_key: &str,
) -> Result<BalanceSnapshot> {
    let value = get_value(transport, &endpoint(base_url, SUB2API_USAGE_PATH)?, api_key).await?;
    let unlimited = bool_at(&value, &["quota", "unlimited"])
        .or_else(|| bool_at(&value, &["unlimited"]))
        .unwrap_or(false);
    let amount = number_at(&value, &["remaining"])
        .or_else(|| number_at(&value, &["balance"]))
        .or_else(|| number_at(&value, &["quota", "remaining"]));
    if !unlimited && amount.is_none() {
        return Err(CoreError::Serialization(
            "sub2api balance has no remaining amount".into(),
        ));
    }
    let unit = string_at(&value, &["unit"])
        .or_else(|| string_at(&value, &["quota", "unit"]))
        .unwrap_or_default();
    Ok(BalanceSnapshot {
        amount: BalanceAmount {
            amount: if unlimited { None } else { amount },
            unit,
            unlimited,
        },
        available: Some(unlimited || amount.is_some_and(|value| value > 0.0)),
    })
}

async fn get_value(transport: Arc<dyn HttpTransport>, url: &str, api_key: &str) -> Result<Value> {
    let body = transport.get_json(url, api_key).await?;
    serde_json::from_str(&body)
        .map_err(|error| CoreError::Serialization(format!("parse balance response: {error}")))
}

fn endpoint(base_url: &str, path: &str) -> Result<String> {
    let mut url = Url::parse(base_url.trim_end_matches('/'))
        .map_err(|_| CoreError::invalid("provider base URL is invalid"))?;
    let base_path = url.path().trim_end_matches('/');
    let next_path = if base_path.is_empty() {
        format!("/{}", path.trim_start_matches('/'))
    } else {
        format!("{}/{}", base_path, path.trim_start_matches('/'))
    };
    url.set_path(&next_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn endpoint_without_v1(base_url: &str, path: &str) -> Result<String> {
    let mut url = Url::parse(base_url.trim_end_matches('/'))
        .map_err(|_| CoreError::invalid("provider base URL is invalid"))?;
    let mut base_path = url.path().trim_end_matches('/').to_owned();
    if base_path == "/v1" {
        base_path.clear();
    }
    let next_path = if base_path.is_empty() {
        format!("/{}", path.trim_start_matches('/'))
    } else {
        format!("{}/{}", base_path, path.trim_start_matches('/'))
    };
    url.set_path(&next_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .filter(|value| value.is_finite())
}

fn amount_micros(amount: f64) -> Option<i64> {
    let micros = (amount * 1_000_000.0).round();
    (micros.is_finite() && micros >= i64::MIN as f64 && micros < i64::MAX as f64)
        .then_some(micros as i64)
}

fn subtract_amounts(limit: Option<f64>, used: Option<f64>) -> Option<f64> {
    let (limit, used) = (amount_micros(limit?)?, amount_micros(used?)?);
    limit
        .checked_sub(used)
        .map(|value| value.max(0) as f64 / 1_000_000.0)
}

fn number_at(value: &Value, path: &[&str]) -> Option<f64> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(|value| number(Some(value)))
}

fn bool_at(value: &Value, path: &[&str]) -> Option<bool> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(Value::as_bool)
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn scrub_balance_error(error: &CoreError, api_key: &str) -> String {
    error.to_string().replace(api_key, "[redacted]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RoutingTransport {
        responses: HashMap<String, String>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl HttpTransport for RoutingTransport {
        async fn stream(
            &self,
            _request: crate::transport::TransportRequest,
            _sink: &mut dyn crate::transport::EventSink,
        ) -> Result<()> {
            Err(CoreError::other("balance test transport does not stream"))
        }

        async fn get_json(&self, url: &str, _api_key: &str) -> Result<String> {
            self.calls.lock().unwrap().push(url.to_owned());
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| CoreError::other(format!("no balance fixture for {url}")))
        }
    }

    #[tokio::test]
    async fn parses_official_shape() {
        let body = r#"{
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "110.50",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.50"
                }
            ]
        }"#;
        let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(body));
        let resp = fetch_balance(transport, "https://api.deepseek.com", "sk-test")
            .await
            .unwrap();
        assert!(resp.is_available);
        assert_eq!(resp.balance_infos.len(), 1);
        let info = &resp.balance_infos[0];
        assert_eq!(info.currency, "CNY");
        assert_eq!(info.total_balance, "110.50");
        assert_eq!(info.granted_balance, "10.00");
        assert_eq!(info.topped_up_balance, "100.50");
    }

    #[tokio::test]
    async fn empty_key_rejected_before_network() {
        let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json("{}"));
        let err = fetch_balance(transport, "https://api.deepseek.com", "   ")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }

    #[tokio::test]
    async fn malformed_body_errors() {
        let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json("not json"));
        assert!(
            fetch_balance(transport, "https://api.deepseek.com", "sk-test")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn missing_fields_default_safely() {
        // Provider could omit fields; defaults must not panic.
        let body = r#"{}"#;
        let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(body));
        let resp = fetch_balance(transport, "https://api.deepseek.com", "sk-test")
            .await
            .unwrap();
        assert!(!resp.is_available);
        assert!(resp.balance_infos.is_empty());
    }

    #[tokio::test]
    async fn official_empty_rows_keep_the_amount_unknown() {
        let body = r#"{"is_available":false,"balance_infos":[]}"#;
        let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(body));
        let result = fetch_balance_by_kind(
            transport,
            "https://api.deepseek.com/v1",
            "sk-test",
            BalanceKind::Official,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.amount, None);
        assert_eq!(result.amount.unit, "CNY");
        assert!(!result.amount.unlimited);
        assert_eq!(result.available, Some(false));
    }

    #[tokio::test]
    async fn none_does_not_call_transport_and_has_no_fake_amount() {
        let result = fetch_balance_by_kind(
            Arc::new(MockTransport::with_get_json("not used")),
            "https://provider.example/v1",
            "",
            BalanceKind::None,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.amount, None);
        assert!(!result.amount.unlimited);
    }

    #[tokio::test]
    async fn sub2api_preserves_unlimited_without_max_integer() {
        let body = r#"{"quota":{"unlimited":true,"unit":"USD"}}"#;
        let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(body));
        let result = fetch_balance_by_kind(
            transport,
            "https://provider.example/v1",
            "sk-test",
            BalanceKind::Sub2Api,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.amount, None);
        assert_eq!(result.amount.unit, "USD");
        assert!(result.amount.unlimited);
    }

    #[tokio::test]
    async fn new_api_routes_all_endpoints_and_keeps_optional_token_usage_optional() {
        let transport = Arc::new(RoutingTransport {
            responses: HashMap::from([
                (
                    "https://relay.example/v1/dashboard/billing/subscription".into(),
                    r#"{"hard_limit_usd":10}"#.into(),
                ),
                (
                    "https://relay.example/v1/dashboard/billing/usage".into(),
                    r#"{"total_usage":250}"#.into(),
                ),
                (
                    "https://relay.example/api/usage/token/".into(),
                    "not-json".into(),
                ),
            ]),
            calls: Mutex::new(Vec::new()),
        });
        let result = fetch_balance_by_kind(
            transport.clone(),
            "https://relay.example/v1",
            "sk-test",
            BalanceKind::NewApi,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.amount, Some(7.5));
        assert_eq!(result.amount.unit, "");
        assert_eq!(result.available, Some(true));
        assert_eq!(
            *transport.calls.lock().unwrap(),
            vec![
                "https://relay.example/v1/dashboard/billing/subscription",
                "https://relay.example/v1/dashboard/billing/usage",
                "https://relay.example/api/usage/token/",
            ]
        );
    }

    #[tokio::test]
    async fn auto_falls_back_to_sub2api_after_new_api_failure() {
        let transport = Arc::new(RoutingTransport {
            responses: HashMap::from([(
                "https://relay.example/v1/usage".into(),
                r#"{"remaining":12.5,"unit":"CNY"}"#.into(),
            )]),
            calls: Mutex::new(Vec::new()),
        });
        let result = fetch_balance_by_kind(
            transport.clone(),
            "https://relay.example/v1",
            "sk-test",
            BalanceKind::Auto,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.amount, Some(12.5));
        assert_eq!(result.amount.unit, "CNY");
        assert_eq!(
            *transport.calls.lock().unwrap(),
            vec![
                "https://relay.example/v1/dashboard/billing/subscription",
                "https://relay.example/v1/usage",
            ]
        );
    }

    #[tokio::test]
    async fn auto_error_contains_both_routes_without_echoing_the_api_key() {
        let transport = Arc::new(RoutingTransport::default());
        let error = fetch_balance_by_kind(
            transport,
            "https://relay.example/v1",
            "sk-secret",
            BalanceKind::Auto,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("PROVIDER_USAGE_DETECT_FAILED"));
        assert!(error.contains("new_api"));
        assert!(error.contains("sub2api"));
        assert!(!error.contains("sk-secret"));
    }

    fn new_api_transport(subscription: Value, usage: Value, token: Value) -> Arc<RoutingTransport> {
        Arc::new(RoutingTransport {
            responses: HashMap::from([
                (
                    "https://relay.example/v1/dashboard/billing/subscription".into(),
                    subscription.to_string(),
                ),
                (
                    "https://relay.example/v1/dashboard/billing/usage".into(),
                    usage.to_string(),
                ),
                (
                    "https://relay.example/api/usage/token/".into(),
                    token.to_string(),
                ),
            ]),
            calls: Mutex::new(Vec::new()),
        })
    }

    #[tokio::test]
    async fn new_api_unknown_shapes_fail_instead_of_inventing_unlimited_quota() {
        for subscription in [
            serde_json::json!({}),
            serde_json::json!({"unlimited_quota":false}),
        ] {
            let result = fetch_balance_by_kind(
                new_api_transport(subscription, serde_json::json!({}), Value::Null),
                "https://relay.example/v1",
                "fixture",
                BalanceKind::NewApi,
            )
            .await;
            assert!(
                result.is_err(),
                "unknown billing shape must not become unlimited: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn new_api_a_limit_without_usage_does_not_fabricate_a_remaining_amount() {
        let result = fetch_balance_by_kind(
            new_api_transport(
                serde_json::json!({"hard_limit_usd":10}),
                serde_json::json!({}),
                Value::Null,
            ),
            "https://relay.example/v1",
            "fixture",
            BalanceKind::NewApi,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.amount, None);
        assert_eq!(result.available, None);
        assert!(!result.amount.unlimited);
    }

    #[tokio::test]
    async fn new_api_token_display_keeps_its_amount_and_unit_together() {
        let result = fetch_balance_by_kind(
            new_api_transport(
                serde_json::json!({"hard_limit_usd":10}),
                serde_json::json!({"total_usage":250}),
                serde_json::json!({"success":true,"data":{"unlimited_quota":false,
                    "display":{"unit":"CNY","remaining":90.750001}}}),
            ),
            "https://relay.example/v1",
            "fixture",
            BalanceKind::NewApi,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.unit, "CNY");
        assert_eq!(result.amount.amount, Some(90.750001));
    }

    #[tokio::test]
    async fn official_messages_base_uses_the_account_balance_endpoint() {
        let transport = Arc::new(RoutingTransport {
            responses: HashMap::from([(
                "https://api.deepseek.com/user/balance".into(),
                r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"1"}]}"#
                    .into(),
            )]),
            calls: Mutex::new(Vec::new()),
        });
        let result = fetch_balance_by_kind(
            transport.clone(),
            "https://api.deepseek.com/anthropic",
            "fixture",
            BalanceKind::Official,
        )
        .await
        .unwrap();
        assert_eq!(result.amount.amount, Some(1.0));
        assert_eq!(
            *transport.calls.lock().unwrap(),
            vec!["https://api.deepseek.com/user/balance"]
        );
    }
}
