//! Canvas namespaces are explicit caller context, not authentication credentials.

use deepagent_core::error::{CoreError, Result};

pub(crate) fn validate_token(value: &str, location: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 120
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':'))
    {
        return Err(CoreError::invalid(format!(
            "{location} must be 1..=120 ASCII letters, digits, underscore, hyphen or colon"
        )));
    }
    Ok(())
}

/// None is the legacy/default namespace, never a wildcard. Explicit invalid
/// identities must not silently fall back to that namespace.
pub(crate) fn normalize_workspace_id(workspace_id: Option<&str>) -> Result<Option<String>> {
    workspace_id
        .map(|value| {
            let value = value.trim();
            validate_token(value, "workspace id")?;
            Ok(value.to_string())
        })
        .transpose()
}
