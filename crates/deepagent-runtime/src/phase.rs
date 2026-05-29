//! The runtime loop phases.
//!
//! Each iteration of the runtime advances through these phases. Modeling them
//! explicitly (rather than as implicit control flow) makes the loop traceable:
//! every phase transition can be observed in the Agent Timeline and is a
//! natural hook point (开发提示词.md §13).

use serde::{Deserialize, Serialize};

/// A single phase of the Agent Runtime Loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopPhase {
    /// Load / recover the session from the event store.
    LoadSession,
    /// Assemble the context (five-layer pipeline).
    BuildContext,
    /// Decide the next high-level move (plan/think).
    Think,
    /// Execute a selected tool.
    Execute,
    /// Observe the environment feedback (开发提示词.md §17).
    Observe,
    /// Verify results (build/test/lint) (开发提示词.md §10).
    Verify,
    /// Reflect on failures and decide whether to retry.
    Reflect,
    /// Compact context to control token growth.
    Compact,
    /// Persist events for this iteration.
    Save,
}

impl LoopPhase {
    /// The canonical ordering of phases within one iteration.
    pub const ORDER: [LoopPhase; 9] = [
        LoopPhase::LoadSession,
        LoopPhase::BuildContext,
        LoopPhase::Think,
        LoopPhase::Execute,
        LoopPhase::Observe,
        LoopPhase::Verify,
        LoopPhase::Reflect,
        LoopPhase::Compact,
        LoopPhase::Save,
    ];

    /// Human-readable label.
    pub const fn label(&self) -> &'static str {
        match self {
            LoopPhase::LoadSession => "load_session",
            LoopPhase::BuildContext => "build_context",
            LoopPhase::Think => "think",
            LoopPhase::Execute => "execute",
            LoopPhase::Observe => "observe",
            LoopPhase::Verify => "verify",
            LoopPhase::Reflect => "reflect",
            LoopPhase::Compact => "compact",
            LoopPhase::Save => "save",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_has_all_phases() {
        assert_eq!(LoopPhase::ORDER.len(), 9);
        assert_eq!(LoopPhase::ORDER[0], LoopPhase::LoadSession);
        assert_eq!(LoopPhase::ORDER[8], LoopPhase::Save);
    }

    #[test]
    fn labels_roundtrip_through_serde() {
        for phase in LoopPhase::ORDER {
            let json = serde_json::to_string(&phase).unwrap();
            assert!(json.contains(phase.label()));
        }
    }
}
