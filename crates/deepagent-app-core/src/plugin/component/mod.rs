//! Component discovery at the fixed locations Agent Plugins v1 defines (§6, §7).
//!
//! v1 standardizes exactly two component types, each at a location a manifest
//! cannot relocate:
//!
//! | Component   | Fixed location | Pattern                              |
//! | ----------- | -------------- | ------------------------------------ |
//! | Skills      | `skills/`      | Subdirectories containing `SKILL.md` |
//! | MCP servers | `mcp.json`     | JSON configuration                   |
//!
//! Everything else — commands, agents, hooks, output styles, sidebar apps — is
//! outside v1 and reaches us through a client dialect or our own extension
//! namespace.
//!
//! Two rules from §6.2 apply uniformly to every discoverer here: a missing fixed
//! location is not an error, and a location present with the wrong filesystem
//! kind invalidates only that component type while the others keep loading.

pub mod skills;

pub use skills::{discover_skills, SkillComponent, SKILL_MANIFEST_FILE_NAME};
