//! DeepSeek account balance query (`GET /user/balance`).
//!
//! Mirrors the official endpoint described at
//! <https://api-docs.deepseek.com/api/get-user-balance>. The response carries an
//! `is_available` flag plus a per-currency breakdown (granted credit + topped-up
//! credit + total). Transport-agnostic — every call goes through
//! [`HttpTransport::get_json`], so tests run offline via the mock transport.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};

use crate::transport::HttpTransport;

/// Endpoint path appended to the DeepSeek base URL.
pub const BALANCE_PATH: &str = "/user/balance";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

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
}
