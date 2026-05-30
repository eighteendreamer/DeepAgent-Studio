//! Session run modes — "运行模式是一等公民"（Claude Code 复刻规范 §5, 原则 1）.
//!
//! Claude Code treats the *run mode* of a session as a first-class concept,
//! dispatched at the entry point: a normal interactive session, a resumed one,
//! a remote/teleport session, a direct-connect to an agent server, a read-only
//! assistant viewer, a coordinator (multi-agent orchestration) session, or a
//! headless background task. The mode determines tool visibility, permission
//! posture, and how the session is driven — so it belongs in the durable
//! session metadata, not as an afterthought.

use serde::{Deserialize, Serialize};

/// How a session is being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// A normal local interactive session.
    #[default]
    Normal,
    /// A session resumed from a prior transcript (replayed then continued).
    Resumed,
    /// A remote / teleport session driven over the network.
    Remote,
    /// A direct connection to a remote agent server (created via `POST /sessions`).
    DirectConnect,
    /// A read-only viewer onto another (usually remote) session.
    AssistantViewer,
    /// A coordinator session that decomposes work and dispatches sub-agents.
    Coordinator,
    /// A headless background task (no interactive UI; SDK / scheduled).
    BackgroundTask,
}

impl SessionMode {
    /// Stable string label (matches the serde representation).
    pub const fn label(&self) -> &'static str {
        match self {
            SessionMode::Normal => "normal",
            SessionMode::Resumed => "resumed",
            SessionMode::Remote => "remote",
            SessionMode::DirectConnect => "direct_connect",
            SessionMode::AssistantViewer => "assistant_viewer",
            SessionMode::Coordinator => "coordinator",
            SessionMode::BackgroundTask => "background_task",
        }
    }

    /// Whether this mode is read-only (no tool execution / mutation allowed).
    /// An assistant viewer only observes; everything else may act.
    pub const fn is_read_only(&self) -> bool {
        matches!(self, SessionMode::AssistantViewer)
    }

    /// Whether this mode is driven over the network rather than locally.
    pub const fn is_remote(&self) -> bool {
        matches!(
            self,
            SessionMode::Remote | SessionMode::DirectConnect | SessionMode::AssistantViewer
        )
    }

    /// Whether this mode is interactive (has a user driving it in real time).
    /// Background tasks and coordinators run autonomously.
    pub const fn is_interactive(&self) -> bool {
        !matches!(self, SessionMode::BackgroundTask | SessionMode::Coordinator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_normal() {
        assert_eq!(SessionMode::default(), SessionMode::Normal);
    }

    #[test]
    fn labels_match_serde() {
        for mode in [
            SessionMode::Normal,
            SessionMode::Resumed,
            SessionMode::Remote,
            SessionMode::DirectConnect,
            SessionMode::AssistantViewer,
            SessionMode::Coordinator,
            SessionMode::BackgroundTask,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.label()));
        }
    }

    #[test]
    fn read_only_only_for_viewer() {
        assert!(SessionMode::AssistantViewer.is_read_only());
        assert!(!SessionMode::Normal.is_read_only());
        assert!(!SessionMode::Remote.is_read_only());
    }

    #[test]
    fn remote_classification() {
        assert!(SessionMode::Remote.is_remote());
        assert!(SessionMode::DirectConnect.is_remote());
        assert!(SessionMode::AssistantViewer.is_remote());
        assert!(!SessionMode::Normal.is_remote());
    }

    #[test]
    fn interactivity_classification() {
        assert!(SessionMode::Normal.is_interactive());
        assert!(!SessionMode::BackgroundTask.is_interactive());
        assert!(!SessionMode::Coordinator.is_interactive());
    }
}
