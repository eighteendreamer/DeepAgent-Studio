//! Five user-controlled reasoning levels. Route capabilities own wire mappings.
//! Reference: Tianshu-harness/src/api/provider.ts (resolution and equivalent choices).

use std::collections::{BTreeMap, HashSet};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    pub const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Max];

    /// Read old persisted seven-level selections without exposing extra UI stops.
    pub const fn canonical(self) -> Self {
        match self {
            Self::Minimal => Self::Low,
            Self::Xhigh => Self::Max,
            other => other,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl From<crate::ReasoningEffort> for ThinkingLevel {
    fn from(value: crate::ReasoningEffort) -> Self {
        match value {
            crate::ReasoningEffort::None => Self::Off,
            crate::ReasoningEffort::Minimal => Self::Minimal,
            crate::ReasoningEffort::Low => Self::Low,
            crate::ReasoningEffort::Medium => Self::Medium,
            crate::ReasoningEffort::High => Self::High,
            crate::ReasoningEffort::Xhigh => Self::Xhigh,
            crate::ReasoningEffort::Max => Self::Max,
        }
    }
}

impl From<ThinkingLevel> for crate::ReasoningEffort {
    fn from(value: ThinkingLevel) -> Self {
        match value {
            ThinkingLevel::Off => Self::None,
            ThinkingLevel::Minimal => Self::Minimal,
            ThinkingLevel::Low => Self::Low,
            ThinkingLevel::Medium => Self::Medium,
            ThinkingLevel::High => Self::High,
            ThinkingLevel::Xhigh => Self::Xhigh,
            ThinkingLevel::Max => Self::Max,
        }
    }
}

/// False disables controls; a nonempty map declares exactly the offered levels.
/// Only off may map to null (omit effort). Missing keys are unsupported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ReasoningEfforts {
    #[default]
    Disabled,
    Levels(BTreeMap<ThinkingLevel, Option<String>>),
}

impl Serialize for ReasoningEfforts {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Disabled => serializer.serialize_bool(false),
            Self::Levels(levels) => levels.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ReasoningEfforts {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if value == Value::Bool(false) {
            return Ok(Self::Disabled);
        }
        let mut levels: BTreeMap<ThinkingLevel, Option<String>> = serde_json::from_value(value)
            .map_err(|_| {
                serde::de::Error::custom("reasoning_efforts must be false or a level map")
            })?;
        if !levels.keys().any(|level| *level != ThinkingLevel::Off)
            || levels.iter().any(|(level, wire)| match wire {
                None => *level != ThinkingLevel::Off,
                Some(wire) => wire.trim().is_empty(),
            })
        {
            return Err(serde::de::Error::custom(
                "reasoning_efforts needs an enabled level and nonempty wire values; only off may be null",
            ));
        }
        for alias in [ThinkingLevel::Minimal, ThinkingLevel::Xhigh] {
            if let Some(wire) = levels.remove(&alias) {
                levels.entry(alias.canonical()).or_insert(wire);
            }
        }
        Ok(Self::Levels(levels))
    }
}

/// Protocol transport shape, separate from the neutral effort value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingFormat {
    #[default]
    EffortOnly,
    Enabled,
    Adaptive,
    Budget,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThinkingCapabilities {
    pub reasoning_efforts: ReasoningEfforts,
    #[serde(default)]
    pub thinking_format: ThinkingFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<ThinkingLevel>,
}

impl ThinkingCapabilities {
    /// Read the override without conflating null, false and omission.
    pub fn declaration(value: &Value) -> Result<Option<ReasoningEfforts>> {
        value
            .as_object()
            .ok_or_else(|| CoreError::invalid("registry model capabilities must be an object"))?;
        value
            .get("reasoning_efforts")
            .map(|efforts| {
                serde_json::from_value(efforts.clone()).map_err(|error| {
                    CoreError::invalid(format!("invalid reasoning_efforts: {error}"))
                })
            })
            .transpose()
    }

    pub fn from_registry_json(value: &Value) -> Result<Self> {
        Self::for_model("", value)
    }

    pub fn for_model(model_id: &str, value: &Value) -> Result<Self> {
        Self::for_route("", model_id, crate::WireMode::Responses, value)
    }

    pub fn for_route(
        base_url: &str,
        model_id: &str,
        mode: crate::WireMode,
        value: &Value,
    ) -> Result<Self> {
        let id = model_id
            .rsplit('/')
            .next()
            .unwrap_or(model_id)
            .to_ascii_lowercase();
        let host = url::Url::parse(base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        // The official DeepSeek Messages endpoint accepts Claude aliases but
        // routes them to DeepSeek. Explicit model declarations still win.
        let official_deepseek = host.as_deref() == Some("api.deepseek.com");
        let profile = if official_deepseek {
            "deepseek-route"
        } else {
            &id
        };
        let mut thinking = Self::resolve_efforts(profile, value)?;
        let deepseek = official_deepseek || id.starts_with("deepseek-");
        thinking.thinking_format = match mode {
            crate::WireMode::Responses => ThinkingFormat::EffortOnly,
            crate::WireMode::ChatCompletions
                if deepseek || id.starts_with("kimi-") || id.starts_with("glm-") =>
            {
                ThinkingFormat::Enabled
            }
            crate::WireMode::ChatCompletions => ThinkingFormat::EffortOnly,
            crate::WireMode::Messages if deepseek => ThinkingFormat::Enabled,
            crate::WireMode::Messages if adaptive_claude(&id) => ThinkingFormat::Adaptive,
            crate::WireMode::Messages => ThinkingFormat::Budget,
        };
        if let Some(format) = value.get("thinking_format") {
            thinking.thinking_format = serde_json::from_value(format.clone())
                .map_err(|_| CoreError::invalid("invalid reasoning transport format"))?;
        }
        if thinking.thinking_format == ThinkingFormat::Budget {
            if let Some(limit) = value.get("max_output_tokens").and_then(Value::as_u64) {
                thinking.merge_budget_choices(
                    u32::try_from(limit)
                        .map_err(|_| CoreError::invalid("invalid thinking output limit"))?,
                )?;
            }
        }
        // A configured application default is an explicit choice, never API omission.
        thinking.default_effort = if let Some(default) = value.get("reasoning_default") {
            let level: ThinkingLevel = serde_json::from_value(default.clone())
                .map_err(|_| CoreError::invalid("invalid reasoning default level"))?;
            Some(thinking.normalize(level)?)
        } else if thinking.levels().is_empty() {
            None
        } else {
            Some(thinking.normalize(ThinkingLevel::High).unwrap_or_else(|_| {
                let levels = thinking.distinct_levels();
                levels
                    .iter()
                    .copied()
                    .find(|level| *level != ThinkingLevel::Off)
                    .unwrap_or(levels[0])
            }))
        };
        Ok(thinking)
    }

    fn resolve_efforts(model_id: &str, value: &Value) -> Result<Self> {
        if let Some(reasoning_efforts) = Self::declaration(value)? {
            return Ok(Self {
                reasoning_efforts,
                default_effort: None,
                thinking_format: ThinkingFormat::default(),
            });
        }
        // Read older saved limits at this boundary. New writes contain only
        // reasoning_efforts; the abandoned family table is never consulted.
        if value.get("thinking_levels").is_some() {
            #[derive(Deserialize)]
            struct Legacy {
                thinking_levels: Vec<ThinkingLevel>,
                #[serde(default)]
                thinking_distinct: Vec<ThinkingLevel>,
                #[serde(default = "yes")]
                thinking_disable: bool,
            }
            fn yes() -> bool {
                true
            }
            let legacy: Legacy = serde_json::from_value(value.clone())
                .map_err(|_| CoreError::invalid("invalid legacy thinking capabilities"))?;
            let unique: HashSet<_> = legacy.thinking_levels.iter().collect();
            if unique.len() != legacy.thinking_levels.len()
                || legacy
                    .thinking_distinct
                    .iter()
                    .collect::<HashSet<_>>()
                    .len()
                    != legacy.thinking_distinct.len()
                || legacy
                    .thinking_distinct
                    .iter()
                    .any(|level| !unique.contains(level))
            {
                return Err(CoreError::invalid(
                    "invalid legacy thinking capability levels",
                ));
            }
            let levels = legacy
                .thinking_levels
                .into_iter()
                .filter(|level| *level != ThinkingLevel::Off || legacy.thinking_disable)
                .map(|level| {
                    (
                        level.canonical(),
                        Some(
                            if level == ThinkingLevel::Off {
                                "none"
                            } else {
                                level.label()
                            }
                            .into(),
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            return Ok(Self::from_levels(levels));
        }
        if let Some(effort) = value.get("effort").filter(|value| !value.is_null()) {
            let metadata: crate::discovery::ModelEffortMetadata =
                serde_json::from_value(effort.clone()).map_err(|_| {
                    CoreError::invalid("invalid registry model effort capabilities")
                })?;
            let mut levels = BTreeMap::new();
            for wire in metadata.supported_levels {
                let level = native_level(&wire)
                    .ok_or_else(|| CoreError::invalid("invalid registry model effort level"))?;
                if !levels.contains_key(&level) || wire == level.label() {
                    levels.insert(level, Some(wire));
                }
            }
            if levels.is_empty()
                || metadata.default_level.as_deref().is_some_and(|default| {
                    !native_level(default).is_some_and(|level| levels.contains_key(&level))
                })
            {
                return Err(CoreError::invalid("invalid registry model effort default"));
            }
            if let ReasoningEfforts::Levels(builtin) = builtin_reasoning(model_id) {
                // Keep route aliases only when they map to an advertised value.
                // An explicitly advertised neutral level takes precedence.
                for (level, wire) in &builtin {
                    if !levels.contains_key(level) && levels.values().any(|value| value == wire) {
                        levels.insert(*level, wire.clone());
                    }
                }
                // Existing routes may declare a separate off control independently
                // of the positive levels discovered from the model catalog.
                if let Some(wire) = builtin.get(&ThinkingLevel::Off) {
                    levels
                        .entry(ThinkingLevel::Off)
                        .or_insert_with(|| wire.clone());
                }
            }
            return Ok(Self::from_levels(levels));
        }
        Ok(Self {
            reasoning_efforts: builtin_reasoning(model_id),
            default_effort: None,
            thinking_format: ThinkingFormat::default(),
        })
    }

    fn from_levels(levels: BTreeMap<ThinkingLevel, Option<String>>) -> Self {
        Self {
            default_effort: None,
            thinking_format: ThinkingFormat::default(),
            reasoning_efforts: if levels.is_empty() {
                ReasoningEfforts::Disabled
            } else {
                ReasoningEfforts::Levels(levels)
            },
        }
    }

    fn merge_budget_choices(&mut self, max_tokens: u32) -> Result<()> {
        let mut representatives = BTreeMap::new();
        let mut mappings = Vec::new();
        for level in self.levels() {
            let effective = self.normalize(level)?;
            if effective == ThinkingLevel::Off {
                continue;
            }
            let budget = thinking_budget(effective, max_tokens)?;
            let representative = *representatives.entry(budget).or_insert(effective);
            mappings.push((level, representative));
        }
        if let ReasoningEfforts::Levels(levels) = &mut self.reasoning_efforts {
            for (level, representative) in mappings {
                levels.insert(level, Some(representative.label().into()));
            }
        }
        Ok(())
    }

    pub fn levels(&self) -> Vec<ThinkingLevel> {
        ThinkingLevel::ALL
            .into_iter()
            .filter(|level| self.allows(*level))
            .collect()
    }

    pub fn allows(&self, level: ThinkingLevel) -> bool {
        matches!(&self.reasoning_efforts, ReasoningEfforts::Levels(levels) if levels.contains_key(&level))
    }

    pub fn wire_effort(&self, level: ThinkingLevel) -> Result<Option<&str>> {
        if let ReasoningEfforts::Levels(levels) = &self.reasoning_efforts {
            if let Some(wire) = levels.get(&level) {
                return Ok(wire.as_deref());
            }
        }
        Err(CoreError::invalid(format!(
            "does not support reasoning effort '{}'",
            level.label()
        )))
    }

    /// Choose one representative for each actual wire value. Prefer its real
    /// neutral spelling (medium→high and max→high must keep High, not Medium).
    pub fn normalize(&self, level: ThinkingLevel) -> Result<ThinkingLevel> {
        let level = level.canonical();
        let wire = self.wire_effort(level)?;
        Ok(self
            .levels()
            .into_iter()
            .find(|candidate| {
                self.wire_effort(*candidate).ok() == Some(wire)
                    && wire
                        == Some(if *candidate == ThinkingLevel::Off {
                            "none"
                        } else {
                            candidate.label()
                        })
            })
            .or_else(|| {
                self.levels()
                    .into_iter()
                    .find(|candidate| self.wire_effort(*candidate).ok() == Some(wire))
            })
            .unwrap_or(level))
    }

    pub fn distinct_levels(&self) -> Vec<ThinkingLevel> {
        self.levels()
            .into_iter()
            .filter(|level| self.normalize(*level).ok() == Some(*level))
            .collect()
    }

    pub(crate) fn chat_thinking(
        &self,
        effort: Option<crate::ReasoningEffort>,
    ) -> Result<Option<crate::chat::ThinkingToggle>> {
        if self.thinking_format == ThinkingFormat::EffortOnly {
            return Ok(None);
        }
        effort
            .map(|effort| {
                let kind = if self.normalize(effort.into())? == ThinkingLevel::Off {
                    "disabled"
                } else if self.thinking_format == ThinkingFormat::Adaptive {
                    "adaptive"
                } else {
                    "enabled"
                };
                Ok(crate::chat::ThinkingToggle { kind: kind.into() })
            })
            .transpose()
    }
}

/// Shared by capability projection and Messages encoding, including small output
/// limits where several neutral levels produce the same actual token budget.
pub(crate) fn thinking_budget(level: ThinkingLevel, max_tokens: u32) -> Result<u32> {
    if max_tokens <= 1024 {
        return Err(CoreError::invalid(
            "thinking budget requires max_output_tokens greater than 1024",
        ));
    }
    Ok(match level.canonical() {
        ThinkingLevel::Low => (max_tokens / 8).min(8192),
        ThinkingLevel::Medium => (u64::from(max_tokens) * 3 / 10) as u32,
        ThinkingLevel::High => (u64::from(max_tokens) * 6 / 10) as u32,
        _ => max_tokens - 1,
    }
    .max(1024)
    .min(max_tokens - 1))
}

fn native_level(wire: &str) -> Option<ThinkingLevel> {
    match wire {
        "none" | "off" => Some(ThinkingLevel::Off),
        "minimal" | "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" | "max" | "ultra" => Some(ThinkingLevel::Max),
        _ => None,
    }
}

// Explicit current Anthropic model generations, rather than treating every
// Claude-compatible identifier as having identical thinking capabilities.
// Primary source: platform.claude.com/docs/en/build-with-claude/thinking.
fn adaptive_claude(id: &str) -> bool {
    [
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-haiku-5",
        "claude-fable-5-2",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ]
    .iter()
    .any(|prefix| id.starts_with(prefix))
}

fn always_thinking_claude(id: &str) -> bool {
    [
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-haiku-5",
        "claude-fable-5-2",
        "claude-haiku-4-5",
    ]
    .iter()
    .any(|prefix| id.starts_with(prefix))
}

/// Reference profiles are isolated here; explicit model declarations win.
/// Compatible gateways retain the chosen model's mapping. Unknown compatible
/// endpoints get explicit standard positive efforts and can override the map.
fn builtin_reasoning(model_id: &str) -> ReasoningEfforts {
    use ThinkingLevel::*;
    let id = model_id
        .rsplit('/')
        .next()
        .unwrap_or(model_id)
        .to_ascii_lowercase();
    let mut map: BTreeMap<_, _> = [
        (Low, "low"),
        (Medium, "medium"),
        (High, "high"),
        (Max, "max"),
    ]
    .into_iter()
    .map(|(level, wire)| (level, Some(wire.into())))
    .collect();
    if id.starts_with("deepseek-") || id.starts_with("kimi-") {
        map.insert(Off, Some("none".into()));
        map.insert(Medium, Some("high".into()));
    } else if id.starts_with("grok-") {
        map.insert(Off, Some("low".into()));
        map.insert(Max, Some("xhigh".into()));
    } else if id.starts_with("claude-") {
        map.insert(
            Off,
            Some(
                if always_thinking_claude(&id) {
                    "low"
                } else {
                    "none"
                }
                .into(),
            ),
        );
        if !always_thinking_claude(&id) && !id.starts_with("claude-opus-") {
            map.insert(Max, Some("high".into()));
        }
    } else if id.starts_with("step-") {
        map.insert(Off, Some("low".into()));
        map.insert(Max, Some("high".into()));
    } else if id.starts_with("glm-") || id.starts_with("doubao-seed-") {
        map.insert(Off, Some("none".into()));
    }
    ReasoningEfforts::Levels(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn h029_controls_use_five_levels_and_explicit_application_default() {
        assert_eq!(
            ThinkingLevel::ALL.map(ThinkingLevel::label).to_vec(),
            ["off", "low", "medium", "high", "max"]
        );
        let unknown = ThinkingCapabilities::for_model("custom-reasoning", &json!({})).unwrap();
        assert!(!unknown.distinct_levels().is_empty());
        assert_eq!(unknown.default_effort, Some(ThinkingLevel::High));
    }

    #[test]
    fn h029_equivalent_choices_keep_the_actual_high_level() {
        let thinking = ThinkingCapabilities::for_model("m", &json!({
            "reasoning_efforts":{"off":"none","low":"low","medium":"high","high":"high","max":"high"}
        })).unwrap();
        assert_eq!(
            thinking.distinct_levels(),
            [ThinkingLevel::Off, ThinkingLevel::Low, ThinkingLevel::High]
        );
    }

    #[test]
    fn h029_official_route_and_current_claude_constraints_override_generic_family_rules() {
        let alias = ThinkingCapabilities::for_route(
            "https://api.deepseek.com/anthropic",
            "claude-opus-5-5",
            crate::WireMode::Messages,
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            alias.wire_effort(ThinkingLevel::Medium).unwrap(),
            Some("high")
        );
        assert_eq!(alias.thinking_format, ThinkingFormat::Enabled);
        for model in ["claude-opus-5-5", "claude-fable-5-2", "claude-haiku-5-5"] {
            let caps = ThinkingCapabilities::for_route(
                "https://api.layso.ai/v1",
                model,
                crate::WireMode::Messages,
                &json!({}),
            )
            .unwrap();
            assert_eq!(
                caps.wire_effort(ThinkingLevel::Max).unwrap(),
                Some("max"),
                "{model}"
            );
            assert_eq!(
                caps.normalize(ThinkingLevel::Off).unwrap(),
                ThinkingLevel::Low,
                "{model}"
            );
            assert!(
                !caps.distinct_levels().contains(&ThinkingLevel::Off),
                "{model}"
            );
        }
        let legacy = ThinkingCapabilities::for_route(
            "https://gateway.test/v1",
            "claude-sonnet-4-5",
            crate::WireMode::Messages,
            &json!({}),
        )
        .unwrap();
        assert_eq!(legacy.thinking_format, ThinkingFormat::Budget);
    }

    #[test]
    fn h029_budget_choices_merge_the_actual_clamped_token_budgets() {
        let caps = ThinkingCapabilities::for_route(
            "https://gateway.test/v1", "claude-sonnet-4-5", crate::WireMode::Messages,
            &json!({"max_output_tokens":2048,"reasoning_efforts":{"off":"none","low":"low","medium":"medium","high":"high","max":"max"}}),
        ).unwrap();
        assert_eq!(
            caps.distinct_levels(),
            [
                ThinkingLevel::Off,
                ThinkingLevel::Low,
                ThinkingLevel::High,
                ThinkingLevel::Max
            ]
        );
        assert_eq!(
            caps.normalize(ThinkingLevel::Medium).unwrap(),
            ThinkingLevel::Low
        );
    }

    #[test]
    fn catalog_inheritance_is_distinct_from_disable_and_unknown_metadata() {
        let known = ThinkingCapabilities::for_model("deepseek-flash", &json!({})).unwrap();
        assert_eq!(
            known.distinct_levels(),
            [
                ThinkingLevel::Off,
                ThinkingLevel::Low,
                ThinkingLevel::High,
                ThinkingLevel::Max
            ]
        );
        assert!(known.wire_effort(ThinkingLevel::Minimal).is_err());
        assert!(known.wire_effort(ThinkingLevel::Xhigh).is_err());
        let value = json!({"reasoning_efforts":false});
        assert!(ThinkingCapabilities::for_model("deepseek-flash", &value)
            .unwrap()
            .levels()
            .is_empty());
    }

    #[test]
    fn explicit_maps_override_catalog_and_legacy_without_clamping() {
        let value = json!({"reasoning_efforts":{"off":null,"minimal":"tiny","xhigh":"extra","max":"ultra"},
            "thinking_levels":["low"],"effort":{"supported_levels":["high"]}});
        let capability = ThinkingCapabilities::for_model("deepseek-flash", &value).unwrap();
        assert_eq!(
            capability.levels(),
            [ThinkingLevel::Off, ThinkingLevel::Low, ThinkingLevel::Max]
        );
        assert_eq!(
            capability.wire_effort(ThinkingLevel::Max).unwrap(),
            Some("ultra")
        );
        assert_eq!(capability.wire_effort(ThinkingLevel::Off).unwrap(), None);
        assert!(capability.wire_effort(ThinkingLevel::High).is_err());
        let restored: ThinkingCapabilities =
            serde_json::from_value(serde_json::to_value(&capability).unwrap()).unwrap();
        assert_eq!(restored, capability);
    }

    #[test]
    fn legacy_limits_remain_explicit_and_do_not_invent_off() {
        let capability = ThinkingCapabilities::from_registry_json(&json!({"effort":{
            "supported_levels":["minimal","xhigh"],"default_level":"xhigh"}}))
        .unwrap();
        assert_eq!(
            capability.levels(),
            [ThinkingLevel::Low, ThinkingLevel::Max]
        );
        assert!(!capability.allows(ThinkingLevel::Off));
        let legacy = ThinkingCapabilities::from_registry_json(
            &json!({"thinking_levels":["off","high"],"thinking_disable":false}),
        )
        .unwrap();
        assert_eq!(legacy.levels(), [ThinkingLevel::High]);
    }
}
