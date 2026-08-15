//! Agent Plugins schema identifiers, discovery locations, and the three-state
//! `$schema` verdict.
//!
//! Spec: Agent Plugins Specification 1.0.0. Section references below ("§5.2")
//! point at that document.
//!
//! Two rules shape this module:
//!
//! - §5.2 requires clients to select validation rules from a *recognized*
//!   `$schema` value and forbids retrieving a schema while loading a plugin.
//!   The verdict is therefore a pure string comparison against a compile-time
//!   allow list — this module performs no I/O and no network access.
//! - §10.1 requires `mcp.json` to declare the same specification version as
//!   `plugin.json`. [`schema_version`] extracts that version so the component
//!   layer can enforce the match.

use serde::Deserialize;

/// Canonical plugin manifest schema identifier for Agent Plugins 1.0.0 (§5.2).
pub const AGENT_PLUGIN_SCHEMA_URI: &str =
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// Canonical MCP configuration schema identifier for Agent Plugins 1.0.0 (§7.2.1).
pub const AGENT_PLUGIN_MCP_SCHEMA_URI: &str =
    "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

/// Shared prefix of every published Agent Plugins schema identifier. A
/// `$schema` carrying this prefix targets Agent Plugins; whether *this* build
/// implements the declared version is a separate question (§5.2).
pub const AGENT_PLUGIN_SCHEMA_PREFIX: &str = "https://agent-plugins.org/schemas/";

/// Plugin manifest schema identifiers this build implements.
pub const SUPPORTED_AGENT_PLUGIN_SCHEMA_URIS: &[&str] = &[AGENT_PLUGIN_SCHEMA_URI];

/// MCP configuration schema identifiers this build implements.
pub const SUPPORTED_AGENT_PLUGIN_MCP_SCHEMA_URIS: &[&str] = &[AGENT_PLUGIN_MCP_SCHEMA_URI];

/// The portable manifest location every conformant client must check (§5.1).
pub const AGENT_PLUGIN_MANIFEST_RELATIVE_PATH: &str = "plugin.json";

/// The portable MCP configuration location (§7.2.1). Fixed — a manifest cannot
/// relocate it.
pub const AGENT_PLUGIN_MCP_RELATIVE_PATH: &str = "mcp.json";

/// The fixed skills discovery directory (§7.1).
pub const AGENT_PLUGIN_SKILLS_RELATIVE_PATH: &str = "skills";

/// Client-specific manifest locations beneath a plugin root, in discovery
/// order. Checked only after the portable `plugin.json` yields no Agent
/// Plugins verdict; mirrors the order Codex ships in
/// `codex-rs/exec-server-protocol/src/protocol.rs`.
pub const DISCOVERABLE_MANIFEST_PATHS: &[&str] = &[
    ".codex-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
];

/// DeepAgent's reverse-domain extension namespace (§8). Carries our
/// proprietary manifest data (apps, permissions, runtime preferences) and names
/// our optional top-level extension directory.
pub const DEEPAGENT_EXTENSION_NAMESPACE: &str = "com.deepagent.studio";

/// Whether a `$schema` value targets an Agent Plugins version this build can
/// interpret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStatus {
    /// A canonical identifier this build implements.
    Supported,
    /// Targets Agent Plugins, but not a version this build implements. §5.2
    /// requires rejecting the plugin and reporting the unsupported version.
    Unsupported,
    /// Does not target Agent Plugins at all (absent, unparseable, or some
    /// other vendor's schema). The caller falls back to dialect discovery.
    Unrelated,
}

/// Minimal projection used to read `$schema` without committing to the rest of
/// the document's shape.
#[derive(Debug, Default, Deserialize)]
struct SchemaCarrier {
    #[serde(default, rename = "$schema")]
    schema: Option<String>,
}

/// Reads `$schema` out of a JSON document, or `None` when the document is not
/// a JSON object or carries no `$schema` string.
pub fn read_schema_value(contents: &str) -> Option<String> {
    serde_json::from_str::<SchemaCarrier>(contents).ok()?.schema
}

/// Classifies a `$schema` value against an allow list of canonical identifiers.
fn classify(schema: Option<&str>, supported: &[&str]) -> SchemaStatus {
    let Some(schema) = schema else {
        return SchemaStatus::Unrelated;
    };
    if supported.contains(&schema) {
        SchemaStatus::Supported
    } else if schema.starts_with(AGENT_PLUGIN_SCHEMA_PREFIX) {
        SchemaStatus::Unsupported
    } else {
        SchemaStatus::Unrelated
    }
}

/// Verdict for a plugin manifest document (§5.2).
pub fn schema_status(contents: &str) -> SchemaStatus {
    classify(
        read_schema_value(contents).as_deref(),
        SUPPORTED_AGENT_PLUGIN_SCHEMA_URIS,
    )
}

/// Verdict for an MCP configuration document (§7.2.1).
pub fn mcp_schema_status(contents: &str) -> SchemaStatus {
    classify(
        read_schema_value(contents).as_deref(),
        SUPPORTED_AGENT_PLUGIN_MCP_SCHEMA_URIS,
    )
}

/// Verdict for an already-extracted `$schema` value of a plugin manifest.
pub fn schema_status_of(schema: Option<&str>) -> SchemaStatus {
    classify(schema, SUPPORTED_AGENT_PLUGIN_SCHEMA_URIS)
}

/// Extracts the specification version segment from an Agent Plugins schema
/// identifier, e.g. `1.0.0` from
/// `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`.
///
/// Returns `None` for identifiers outside the Agent Plugins prefix, so callers
/// cannot accidentally compare versions across vendors.
pub fn schema_version(schema: &str) -> Option<&str> {
    let rest = schema.strip_prefix(AGENT_PLUGIN_SCHEMA_PREFIX)?;
    let (version, _) = rest.split_once('/')?;
    (!version.is_empty()).then_some(version)
}

/// Whether an MCP configuration targets the same specification version as its
/// plugin manifest (§10.1). A mismatch invalidates the MCP configuration but
/// leaves other component types loadable (§7.2.2).
pub fn versions_match(manifest_schema: &str, mcp_schema: &str) -> bool {
    match (schema_version(manifest_schema), schema_version(mcp_schema)) {
        (Some(manifest), Some(mcp)) => manifest == mcp,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_manifest_schema_is_supported() {
        let contents = format!(r#"{{"$schema": "{AGENT_PLUGIN_SCHEMA_URI}", "name": "demo"}}"#);
        assert_eq!(schema_status(&contents), SchemaStatus::Supported);
    }

    #[test]
    fn canonical_mcp_schema_is_supported() {
        let contents =
            format!(r#"{{"$schema": "{AGENT_PLUGIN_MCP_SCHEMA_URI}", "mcpServers": {{}}}}"#);
        assert_eq!(mcp_schema_status(&contents), SchemaStatus::Supported);
    }

    #[test]
    fn same_prefix_other_version_is_unsupported() {
        let contents =
            r#"{"$schema": "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json"}"#;
        assert_eq!(schema_status(contents), SchemaStatus::Unsupported);
    }

    #[test]
    fn mcp_identifier_in_manifest_slot_is_unsupported_not_supported() {
        // Right family, wrong document kind: it carries the Agent Plugins
        // prefix but is not an allowed *manifest* identifier.
        let contents = format!(r#"{{"$schema": "{AGENT_PLUGIN_MCP_SCHEMA_URI}"}}"#);
        assert_eq!(schema_status(&contents), SchemaStatus::Unsupported);
    }

    #[test]
    fn foreign_schema_is_unrelated() {
        let contents = r#"{"$schema": "https://json.schemastore.org/package.json"}"#;
        assert_eq!(schema_status(contents), SchemaStatus::Unrelated);
    }

    #[test]
    fn missing_schema_is_unrelated() {
        assert_eq!(
            schema_status(r#"{"name": "demo"}"#),
            SchemaStatus::Unrelated
        );
    }

    #[test]
    fn malformed_json_is_unrelated() {
        assert_eq!(schema_status("{not json"), SchemaStatus::Unrelated);
    }

    #[test]
    fn non_object_document_is_unrelated() {
        assert_eq!(schema_status("[]"), SchemaStatus::Unrelated);
        assert_eq!(schema_status("\"text\""), SchemaStatus::Unrelated);
    }

    #[test]
    fn non_string_schema_is_unrelated() {
        assert_eq!(schema_status(r#"{"$schema": 7}"#), SchemaStatus::Unrelated);
    }

    #[test]
    fn schema_version_extracts_specification_version() {
        assert_eq!(schema_version(AGENT_PLUGIN_SCHEMA_URI), Some("1.0.0"));
        assert_eq!(schema_version(AGENT_PLUGIN_MCP_SCHEMA_URI), Some("1.0.0"));
    }

    #[test]
    fn schema_version_rejects_foreign_identifiers() {
        assert_eq!(
            schema_version("https://example.com/1.0.0/plugin.json"),
            None
        );
        // Prefix present but no trailing path segment to delimit the version.
        assert_eq!(schema_version(AGENT_PLUGIN_SCHEMA_PREFIX), None);
    }

    #[test]
    fn versions_match_requires_same_specification_version() {
        assert!(versions_match(
            AGENT_PLUGIN_SCHEMA_URI,
            AGENT_PLUGIN_MCP_SCHEMA_URI
        ));
        assert!(!versions_match(
            AGENT_PLUGIN_SCHEMA_URI,
            "https://agent-plugins.org/schemas/2.0.0/mcp.schema.json"
        ));
        assert!(!versions_match(
            AGENT_PLUGIN_SCHEMA_URI,
            "https://example.com/mcp.json"
        ));
    }

    #[test]
    fn discovery_order_matches_codex() {
        assert_eq!(
            DISCOVERABLE_MANIFEST_PATHS,
            &[
                ".codex-plugin/plugin.json",
                ".claude-plugin/plugin.json",
                ".cursor-plugin/plugin.json",
            ]
        );
    }
}
