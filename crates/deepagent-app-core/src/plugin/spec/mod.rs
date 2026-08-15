//! Agent Plugins Specification 1.0.0 primitives.
//!
//! This layer owns the *portable* contract only: schema identification, name
//! constraints, plugin-relative path resolution, and placeholder expansion.
//! Client-specific manifest shapes live in [`super::dialect`]; component
//! discovery lives in [`super::component`].
//!
//! Everything here is pure: no filesystem writes, no network. §5.2 forbids
//! retrieving a schema during plugin load, and keeping this layer pure makes
//! that structurally true rather than a convention.

pub mod name;
pub mod schema;

pub use name::{PluginName, PluginNameError, MAX_PLUGIN_NAME_LEN};
pub use schema::{
    mcp_schema_status, read_schema_value, schema_status, schema_status_of, schema_version,
    versions_match, SchemaStatus, AGENT_PLUGIN_MANIFEST_RELATIVE_PATH,
    AGENT_PLUGIN_MCP_RELATIVE_PATH, AGENT_PLUGIN_MCP_SCHEMA_URI, AGENT_PLUGIN_SCHEMA_PREFIX,
    AGENT_PLUGIN_SCHEMA_URI, AGENT_PLUGIN_SKILLS_RELATIVE_PATH, DEEPAGENT_EXTENSION_NAMESPACE,
    DISCOVERABLE_MANIFEST_PATHS, SUPPORTED_AGENT_PLUGIN_MCP_SCHEMA_URIS,
    SUPPORTED_AGENT_PLUGIN_SCHEMA_URIS,
};
