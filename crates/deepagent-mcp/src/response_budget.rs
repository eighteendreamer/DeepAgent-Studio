//! One raw-envelope budget, before UTF-8/JSON/SSE materialization.
use deepagent_core::{CoreError, Result};

/// Default DeepAgent per-message/body budget, not an MCP protocol limit.
pub const DEFAULT_MCP_RESPONSE_LIMIT_BYTES: usize = 16 * 1024 * 1024;
/// Explicit overrides remain finite; use pagination/references above this.
pub const MAX_MCP_RESPONSE_LIMIT_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn validate_limit(limit: usize) -> Result<usize> {
    if !(1..=MAX_MCP_RESPONSE_LIMIT_BYTES).contains(&limit) {
        return Err(CoreError::invalid(
            "MCP response_limit_bytes must be between 1 and 67108864",
        ));
    }
    Ok(limit)
}

pub(crate) fn check_size(observed_bytes: usize, limit_bytes: usize) -> Result<()> {
    if observed_bytes > limit_bytes {
        return Err(CoreError::McpResponseTooLarge {
            limit_bytes,
            observed_bytes,
        });
    }
    Ok(())
}

pub(crate) fn append(bytes: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<()> {
    let required = bytes.len().saturating_add(chunk.len());
    check_size(required, limit)?;
    if required > bytes.capacity() {
        // Geometric growth for fragmented input, clamped to the same raw
        // envelope budget instead of either per-byte reallocations or 2x overflow.
        let capacity = bytes
            .capacity()
            .saturating_mul(2)
            .max(4096)
            .max(required)
            .min(limit);
        bytes
            .try_reserve_exact(capacity - bytes.len())
            .map_err(|_| CoreError::other("MCP bounded response allocation failed"))?;
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn h126_fragmented_buffers_grow_geometrically_without_exceeding_limit() {
        let mut bytes = Vec::new();
        let mut changes = 0;
        for _ in 0..4097 {
            let capacity = bytes.capacity();
            append(&mut bytes, b"x", 8192).unwrap();
            changes += usize::from(bytes.capacity() != capacity);
            assert!(bytes.capacity() <= 8192);
        }
        assert!(
            changes <= 4,
            "fragmented input caused {changes} buffer growth operations"
        );
    }
    #[test]
    fn h126_exact_bytes_not_characters_and_overflow_never_copies() {
        let mut bytes = Vec::new();
        append(&mut bytes, "合成".as_bytes(), 6).unwrap();
        let before = bytes.clone();
        assert!(matches!(
            append(&mut bytes, &[0xff], 6),
            Err(CoreError::McpResponseTooLarge {
                limit_bytes: 6,
                observed_bytes: 7
            })
        ));
        assert_eq!(bytes, before);
        assert!(bytes.capacity() <= 6);
    }
    #[test]
    fn h126_default_and_explicit_limits_round_trip() {
        let default: crate::McpServerConfig =
            serde_json::from_str(r#"{"command":"node"}"#).unwrap();
        assert_eq!(
            default.response_limit().unwrap(),
            DEFAULT_MCP_RESPONSE_LIMIT_BYTES
        );
        assert!(serde_json::to_value(&default)
            .unwrap()
            .get("response_limit_bytes")
            .is_none());
        for limit in [1, MAX_MCP_RESPONSE_LIMIT_BYTES] {
            let c: crate::McpServerConfig = serde_json::from_value(
                serde_json::json!({"command":"node","response_limit_bytes":limit}),
            )
            .unwrap();
            assert_eq!(c.response_limit().unwrap(), limit);
            assert_eq!(
                serde_json::to_value(&c).unwrap()["response_limit_bytes"],
                limit
            );
        }
    }
}
