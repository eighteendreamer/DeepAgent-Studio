//! Plugin subsystem.
//!
//! The internal model is [Agent Plugins Specification 1.0.0][spec], an open,
//! vendor-neutral standard published by a TSC of maintainers from Amazon,
//! Cursor, Microsoft, OpenAI, and Vercel. Claude Code, Codex, and Cursor all
//! support it, so implementing the standard — rather than inventing a private
//! compatibility model — is what makes third-party plugins loadable here and
//! our own plugins loadable elsewhere.
//!
//! [spec]: https://github.com/agentplugins/agent-plugins-spec
//!
//! Layering, from portable core outward:
//!
//! - [`spec`] — the portable contract: `$schema` identification, §5.5 name
//!   constraints, plugin-relative path resolution, placeholder expansion.
//! - `dialect` — client-specific manifest shapes (`.codex-plugin`,
//!   `.claude-plugin`, `.cursor-plugin`) normalized into the portable core.
//! - `component` — discovery of `skills/` and `mcp.json` at their fixed
//!   locations, plus the extended component types the spec leaves to clients.
//!
//! DeepAgent's proprietary capabilities (apps, permissions, runtime
//! preferences) live under the `com.deepagent.studio` extension namespace that
//! §8 reserves for clients, not as new top-level manifest fields.
//!
//! Migration note: the pre-existing `plugin_manifest`, `plugin_loader`,
//! `plugin_marketplace`, `plugin_runtime`, `plugin_security`,
//! `plugin_dependency`, and `plugin_service` modules still hold the shipping
//! implementation. They move into this tree in a dedicated move-only change so
//! the refactor never mixes with behavior changes.

pub mod component;
pub mod dialect;
pub mod model;
pub mod spec;

pub use dialect::{
    discover, DiscoveredManifest, DiscoveredOverlay, DiscoveryError, ManifestDialect,
};
pub use model::{ComponentKind, DiagnosticSeverity, PluginDiagnostic};
