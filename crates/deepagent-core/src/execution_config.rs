//! Explicit execution overrides shared by runtime admission and durable sessions.

use serde::{Deserialize, Serialize};

/// Only explicit overrides are pinned. Absent fields continue to use the
/// runtime's existing settings/project resolution, not a second config engine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<String>,
}

/// Versioned metadata in the existing authoritative session file header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionExecutionConfig {
    pub version: u32,
    pub overrides: ExecutionOverrides,
}

impl SessionExecutionConfig {
    pub fn new(overrides: ExecutionOverrides) -> Self {
        Self {
            version: 1,
            overrides,
        }
    }

    /// Never silently resume an unknown configuration as unrestricted defaults.
    pub fn validate(&self) -> crate::Result<()> {
        if self.version != 1 {
            return Err(crate::CoreError::invalid(format!(
                "unsupported session execution configuration version: {}",
                self.version
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_config_round_trip_and_unsupported_contract_rejection() {
        let mut config = SessionExecutionConfig::new(ExecutionOverrides {
            permission_profile: Some("read_only".into()),
            ..Default::default()
        });
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(
            value["overrides"],
            serde_json::json!({"permission_profile":"read_only"})
        );
        assert_eq!(
            serde_json::from_value::<SessionExecutionConfig>(value).unwrap(),
            config
        );
        config.version = 2;
        assert!(config.validate().is_err());
        assert!(
            serde_json::from_value::<SessionExecutionConfig>(serde_json::json!({
                "version":1, "overrides":{"new_permission_contract":true}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SessionExecutionConfig>(serde_json::json!({
                "version":1, "overrides":{"permission_profile":7}
            }))
            .is_err()
        );
    }
}
