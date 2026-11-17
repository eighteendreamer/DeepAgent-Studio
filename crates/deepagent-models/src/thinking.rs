//! Harness thinking tiers and the single UI-tier → provider-wire mapping table.
//!
//! Plan `docs/2026-11-17_harness-multi-protocol-plan.md` §1.2–§1.3. This module
//! is the only place that decides which wire value a tier produces. Codecs
//! (P2) place the returned value into the right request field; they must not
//! re-derive it.
//!
//! `ReasoningEffort` in `chat.rs` is the legacy four-variant DeepSeek type. It
//! stays until the UI moves to `ThinkingLevel` (P9); until then this table is
//! the single source for the new protocol path.

use serde::{Deserialize, Serialize};

/// Five user-facing tiers (plan §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
    Max,
}

impl ThinkingLevel {
    /// Canonical label used in settings, UI and logs.
    pub const fn label(self) -> &'static str {
        match self {
            ThinkingLevel::Off => "off",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::Max => "max",
        }
    }
}

/// Model families that share one row of the mapping table (plan §1.1 B-class).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingFamily {
    AnthropicMessages,
    OpenAiGpt5,
    DeepSeekV4,
    Glm52,
    Glm53,
    KimiK3,
    KimiK31,
    Qwen38,
    MiMo,
}

/// What a tier means on the wire for one family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingWire {
    /// Send this value in the family's effort field. Field placement belongs
    /// to the codec.
    Effort(&'static str),
    /// Send the family's on/off switch (`enable_thinking`, `thinking.type`, ...).
    Switch(bool),
    /// Send nothing; the family defaults to thinking off (DeepSeek-V4 Flash).
    Omit,
    /// The family cannot express this tier. The UI must grey it out. Sending it
    /// is an error, never a silent downgrade (plan §1.3 footnote ¹).
    Unsupported,
}

/// Plan §1.3 mapping. Footnote ³ (provider-side remapping) means the original
/// tier value is sent unchanged, so `Effort(level.label())` is used for those
/// cells. Explicit clamps (OpenAI `max` → `high`, footnote ⁵) are written out.
pub fn map_thinking(family: ThinkingFamily, level: ThinkingLevel) -> ThinkingWire {
    use ThinkingFamily as F;
    use ThinkingLevel as L;
    use ThinkingWire::{Effort, Omit, Switch, Unsupported};

    match (family, level) {
        (F::AnthropicMessages, L::Off) => Unsupported,
        (F::AnthropicMessages, L::Low) => Effort("low"),
        (F::AnthropicMessages, L::Medium) => Effort("medium"),
        (F::AnthropicMessages, L::High) => Effort("high"),
        (F::AnthropicMessages, L::Max) => Effort("max"),

        (F::OpenAiGpt5, L::Off) => Effort("none"),
        (F::OpenAiGpt5, L::Low) => Effort("low"),
        (F::OpenAiGpt5, L::Medium) => Effort("medium"),
        (F::OpenAiGpt5, L::High) => Effort("high"),
        (F::OpenAiGpt5, L::Max) => Effort("high"),

        (F::DeepSeekV4, L::Off) => Omit,
        (F::DeepSeekV4, L::Low) => Effort("low"),
        (F::DeepSeekV4, L::Medium) => Effort("medium"),
        (F::DeepSeekV4, L::High) => Effort("high"),
        (F::DeepSeekV4, L::Max) => Effort("max"),

        (F::Glm52, L::Off) => Switch(false),
        (F::Glm52, L::Low) => Effort("low"),
        (F::Glm52, L::Medium) => Effort("medium"),
        (F::Glm52, L::High) => Effort("high"),
        (F::Glm52, L::Max) => Effort("max"),

        (F::Glm53, L::Off) => Unsupported,
        (F::Glm53, L::Low) => Effort("low"),
        (F::Glm53, L::Medium) => Effort("medium"),
        (F::Glm53, L::High) => Effort("high"),
        (F::Glm53, L::Max) => Effort("max"),

        (F::KimiK3, L::Off) => Unsupported,
        (F::KimiK3, L::Low) => Effort("low"),
        (F::KimiK3, L::Medium) => Effort("medium"),
        (F::KimiK3, L::High) => Effort("high"),
        (F::KimiK3, L::Max) => Effort("max"),

        (F::KimiK31, L::Off) => Switch(false),
        (F::KimiK31, L::Low) => Effort("low"),
        (F::KimiK31, L::Medium) => Effort("medium"),
        (F::KimiK31, L::High) => Effort("high"),
        (F::KimiK31, L::Max) => Effort("max"),

        (F::Qwen38, L::Off) => Switch(false),
        (F::Qwen38, L::Low) => Effort("low"),
        (F::Qwen38, L::Medium) => Effort("medium"),
        (F::Qwen38, L::High) => Effort("high"),
        (F::Qwen38, L::Max) => Effort("max"),

        (F::MiMo, L::Off) => Switch(false),
        (F::MiMo, _) => Switch(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ThinkingFamily as F;
    use ThinkingLevel as L;
    use ThinkingWire::{Effort, Omit, Switch, Unsupported};

    /// Every cell of plan §1.3, one assertion per (family, tier).
    #[test]
    fn mapping_matches_plan_section_1_3_for_every_cell() {
        let expected: &[(F, [ThinkingWire; 5])] = &[
            // columns: off, low, medium, high, max
            (
                F::AnthropicMessages,
                [
                    Unsupported,
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("max"),
                ],
            ),
            (
                F::OpenAiGpt5,
                [
                    Effort("none"),
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("high"),
                ],
            ),
            (
                F::DeepSeekV4,
                [
                    Omit,
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("max"),
                ],
            ),
            (
                F::Glm52,
                [
                    Switch(false),
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("max"),
                ],
            ),
            (
                F::Glm53,
                [
                    Unsupported,
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("max"),
                ],
            ),
            (
                F::KimiK3,
                [
                    Unsupported,
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("max"),
                ],
            ),
            (
                F::KimiK31,
                [
                    Switch(false),
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("max"),
                ],
            ),
            (
                F::Qwen38,
                [
                    Switch(false),
                    Effort("low"),
                    Effort("medium"),
                    Effort("high"),
                    Effort("max"),
                ],
            ),
            (
                F::MiMo,
                [
                    Switch(false),
                    Switch(true),
                    Switch(true),
                    Switch(true),
                    Switch(true),
                ],
            ),
        ];
        let levels = [L::Off, L::Low, L::Medium, L::High, L::Max];
        for (family, cells) in expected {
            for (level, want) in levels.iter().zip(cells.iter()) {
                assert_eq!(
                    map_thinking(*family, *level),
                    *want,
                    "family {family:?} tier {}",
                    level.label()
                );
            }
        }
    }

    #[test]
    fn unsupported_off_is_never_mapped_to_a_silent_lower_tier() {
        for family in [F::AnthropicMessages, F::Glm53, F::KimiK3] {
            assert_eq!(map_thinking(family, L::Off), Unsupported, "{family:?}");
        }
    }

    #[test]
    fn labels_are_stable_snake_case() {
        let labels: Vec<&str> = [L::Off, L::Low, L::Medium, L::High, L::Max]
            .iter()
            .map(|level| level.label())
            .collect();
        assert_eq!(labels, ["off", "low", "medium", "high", "max"]);
        assert_eq!(serde_json::to_string(&L::Medium).unwrap(), "\"medium\"");
        assert_eq!(
            serde_json::to_string(&F::AnthropicMessages).unwrap(),
            "\"anthropic_messages\""
        );
    }
}
