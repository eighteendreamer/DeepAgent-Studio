//! Agent Plugins Specification 1.0.0 primitives.
//!
//! This layer owns the *portable* contract only: schema identification, name
//! constraints, plugin-relative path resolution, and placeholder expansion.
//! Client-specific manifest shapes live in [`super::dialect`]; component
//! discovery lives in [`super::component`].
//!
//! Everything here is pure with one deliberate exception:
//! [`path::resolve_existing_within`] canonicalizes real paths, because §4.1
//! requires rejecting symlinks that resolve outside the plugin root and that
//! cannot be decided lexically. Nothing here writes, and nothing here reaches
//! the network — §5.2 forbids retrieving a schema during plugin load, and
//! keeping this layer read-only makes that structurally true rather than a
//! convention.

pub mod name;
pub mod path;
pub mod placeholder;
pub mod schema;
pub mod v1;

pub use name::{PluginName, PluginNameError, MAX_PLUGIN_NAME_LEN};
pub use path::{is_within, resolve_existing_within, resolve_plugin_relative, PluginPathError};
pub use placeholder::{
    classify_cwd, expand_v1, normalize_and_expand, reserved_env_key, rewrite_dialect_aliases,
    CwdForm, PLUGIN_DATA_VAR, PLUGIN_ROOT_VAR, RESERVED_ENV_VARS,
};
pub use v1::{parse_portable, PluginAuthor, PortableManifest, PortableManifestError};

pub use schema::{
    mcp_schema_status, read_schema_value, schema_status, schema_status_of, schema_version,
    versions_match, SchemaStatus, AGENT_PLUGIN_MANIFEST_RELATIVE_PATH,
    AGENT_PLUGIN_MCP_RELATIVE_PATH, AGENT_PLUGIN_MCP_SCHEMA_URI, AGENT_PLUGIN_SCHEMA_PREFIX,
    AGENT_PLUGIN_SCHEMA_URI, AGENT_PLUGIN_SKILLS_RELATIVE_PATH, DEEPAGENT_EXTENSION_NAMESPACE,
    DISCOVERABLE_MANIFEST_PATHS, SUPPORTED_AGENT_PLUGIN_MCP_SCHEMA_URIS,
    SUPPORTED_AGENT_PLUGIN_SCHEMA_URIS,
};
