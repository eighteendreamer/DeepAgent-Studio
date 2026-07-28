use std::collections::HashSet;
use std::path::Path;

use deepagent_context::{ConfigLayer, ConfigSource, DualConfigLoader};
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::{HookDefinitions, PermissionRules};

use crate::settings::{
    ApprovalPolicy, EffectivePermissionProfile, LocalExecutionMode, PermissionPreset, SandboxMode,
};

#[derive(Debug, Clone)]
pub(crate) struct RunConfigOverlay {
    pub(crate) value: serde_json::Value,
    pub(crate) sources: Vec<ConfigSource>,
    /// Per-source raw values (ascending precedence). Needed for aggregation
    /// semantics (permission rules) that a last-write-wins merge would break.
    pub(crate) layers: Vec<ConfigLayer>,
    pub(crate) errors: Vec<String>,
}

impl RunConfigOverlay {
    pub(crate) fn load(root: &Path) -> Self {
        let overlay = DualConfigLoader::new(root).load();
        Self {
            value: overlay.value,
            sources: overlay.sources,
            layers: overlay.layers,
            errors: overlay.errors,
        }
    }

    #[cfg(test)]
    fn from_value(value: serde_json::Value) -> Self {
        Self {
            value,
            sources: Vec::new(),
            layers: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[cfg(test)]
    fn from_layers(layers: Vec<ConfigLayer>) -> Self {
        let mut value = serde_json::json!({});
        for layer in &layers {
            merge_json(&mut value, layer.value.clone());
        }
        Self {
            value,
            sources: Vec::new(),
            layers,
            errors: Vec::new(),
        }
    }

    pub(crate) fn apply_permission_profile(
        &self,
        mut profile: EffectivePermissionProfile,
    ) -> Result<EffectivePermissionProfile> {
        if let Some(preset) = string_field(
            &self.value,
            &["permission_preset", "active_permission_preset"],
        ) {
            if let Some(parsed) = PermissionPreset::parse(preset) {
                profile = parsed.to_effective_profile();
            } else {
                return Err(CoreError::invalid(format!(
                    "invalid permission preset in project config: {preset}"
                )));
            }
        }
        if let Some(policy) = parse_overlay_enum::<ApprovalPolicy>(
            &self.value,
            &["approval_policy", "approvalPolicy"],
        )? {
            profile.approval_policy = policy;
        }
        if let Some(sandbox) =
            parse_overlay_enum::<SandboxMode>(&self.value, &["sandbox_mode", "sandboxMode"])?
        {
            profile.sandbox_mode = sandbox;
        }
        profile.local_execution_mode = if matches!(
            (profile.approval_policy, profile.sandbox_mode),
            (ApprovalPolicy::FullAccess, SandboxMode::FullAccess)
        ) {
            LocalExecutionMode::Direct
        } else {
            LocalExecutionMode::SandboxiePreferred
        };
        profile.network_always_ask = matches!(profile.approval_policy, ApprovalPolicy::AlwaysAsk);
        Ok(profile)
    }

    /// Merge permission rules across ALL config layers plus the caller's UI
    /// settings rules (treated as a user-scope source).
    ///
    /// Unlike scalar settings, permission rules NEVER use plain overwrite:
    /// every source's lists are set-unioned, then `deny > ask > allow` is
    /// enforced across the union. Managed (admin) layers are supreme — a
    /// managed `allow` strips conflicting deny/ask from lower scopes, and
    /// managed deny/ask can never be removed by lower scopes.
    pub(crate) fn merged_permission_rules(
        &self,
        ui_rules: PermissionRules,
    ) -> Result<PermissionRules> {
        let mut managed = PermissionRules::default();
        let mut lower = ui_rules;
        for layer in &self.layers {
            let Some(value) = layer
                .value
                .get("permissions")
                .or_else(|| layer.value.get("permission_rules"))
            else {
                continue;
            };
            let rules: PermissionRules =
                serde_json::from_value(value.clone()).map_err(|error| {
                    CoreError::invalid(format!(
                        "invalid permission rules in {}: {error}",
                        layer.path
                    ))
                })?;
            if layer.is_managed() {
                managed = merge_permission_rules(managed, rules);
            } else {
                lower = merge_permission_rules(lower, rules);
            }
        }
        // Managed supremacy: a managed allow cannot be blocked from below.
        let managed_allow = rule_key_set(&managed.allow);
        lower
            .deny
            .retain(|item| !managed_allow.contains(&rule_key(item)));
        lower
            .ask
            .retain(|item| !managed_allow.contains(&rule_key(item)));
        // Managed first so first-wins dedupe keeps the managed spelling; the
        // final precedence pass enforces deny > ask > allow across the union.
        Ok(merge_permission_rules(managed, lower))
    }

    /// Scalar `model` override resolved through the standard precedence
    /// chain (managed > run > local > project > user > plugin).
    pub(crate) fn model_override(&self) -> Option<&str> {
        string_field(&self.value, &["model"])
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// Hooks aggregate across ALL config layers (plugin/user/project/local/
    /// run/managed): every source's handler groups are appended, never
    /// overwritten by a higher scope — hooks from an admin layer and a
    /// project layer for the same event all run.
    pub(crate) fn hook_definitions(&self) -> Result<Option<HookDefinitions>> {
        let mut merged = HookDefinitions::default();
        if self.layers.is_empty() {
            // Overlays built from a single merged value (tests / legacy).
            collect_hook_definitions(&self.value, &mut merged)?;
        } else {
            for layer in &self.layers {
                collect_hook_definitions(&layer.value, &mut merged)?;
            }
        }
        Ok((!merged.is_empty()).then_some(merged))
    }
}

fn collect_hook_definitions(value: &serde_json::Value, merged: &mut HookDefinitions) -> Result<()> {
    if let Some(raw) = string_field(value, &["hooks_json", "hooksJson"]) {
        if !raw.trim().is_empty() {
            crate::plugin_runtime::merge_hook_definitions(merged, HookDefinitions::parse(raw)?);
        }
    }
    if let Some(hooks) = value.get("hooks").filter(|value| value.is_object()) {
        let raw = serde_json::to_string(&serde_json::json!({ "hooks": hooks }))?;
        crate::plugin_runtime::merge_hook_definitions(merged, HookDefinitions::parse(&raw)?);
    }
    Ok(())
}

fn parse_overlay_enum<T>(value: &serde_json::Value, keys: &[&str]) -> Result<Option<T>>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let Some(raw) = keys.iter().find_map(|key| value.get(*key)) else {
        return Ok(None);
    };
    serde_json::from_value(raw.clone())
        .map(Some)
        .map_err(|error| {
            CoreError::invalid(format!(
                "invalid config value for {}: {error}",
                keys.first().copied().unwrap_or("field")
            ))
        })
}

fn string_field<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
}

pub(crate) fn merge_permission_rules(
    mut base: PermissionRules,
    overlay: PermissionRules,
) -> PermissionRules {
    push_unique_strings(&mut base.allow, overlay.allow);
    push_unique_strings(&mut base.ask, overlay.ask);
    push_unique_strings(&mut base.deny, overlay.deny);
    enforce_rule_precedence(&mut base);
    base
}

fn push_unique_strings(target: &mut Vec<String>, incoming: Vec<String>) {
    for item in incoming {
        if !target.iter().any(|existing| same_rule(existing, &item)) {
            target.push(item);
        }
    }
}

fn enforce_rule_precedence(rules: &mut PermissionRules) {
    let deny = rule_key_set(&rules.deny);
    rules.ask.retain(|item| !deny.contains(&rule_key(item)));

    let ask = rule_key_set(&rules.ask);
    rules
        .allow
        .retain(|item| !deny.contains(&rule_key(item)) && !ask.contains(&rule_key(item)));
}

fn rule_key_set(items: &[String]) -> HashSet<String> {
    items.iter().map(|item| rule_key(item)).collect()
}

fn same_rule(left: &str, right: &str) -> bool {
    rule_key(left) == rule_key(right)
}

fn rule_key(item: &str) -> String {
    item.trim().to_ascii_lowercase()
}

/// Deep JSON merge (objects merge per key, everything else overwrites) —
/// same semantics as the loader's scalar overlay. Test-only today.
#[cfg(test)]
fn merge_json(target: &mut serde_json::Value, source: serde_json::Value) {
    match (target, source) {
        (serde_json::Value::Object(target), serde_json::Value::Object(source)) => {
            for (key, value) in source {
                merge_json(target.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
        (target, source) => *target = source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_profile_recomputes_execution_mode_after_overrides() {
        let overlay = RunConfigOverlay::from_value(serde_json::json!({
            "permission_preset": "full_access",
            "approval_policy": "always_ask"
        }));
        let profile = overlay
            .apply_permission_profile(PermissionPreset::Default.to_effective_profile())
            .unwrap();

        assert_eq!(profile.approval_policy, ApprovalPolicy::AlwaysAsk);
        assert_eq!(profile.sandbox_mode, SandboxMode::FullAccess);
        assert_eq!(
            profile.local_execution_mode,
            LocalExecutionMode::SandboxiePreferred
        );
        assert!(profile.network_always_ask);
    }

    #[test]
    fn permission_rule_merge_keeps_highest_precedence_for_same_pattern() {
        let base = PermissionRules::new(
            ["Bash(git:*)".to_string(), "Read".to_string()],
            ["Write".to_string()],
            [],
        );
        let overlay = PermissionRules::new(
            ["Edit".to_string()],
            ["read".to_string()],
            ["write".to_string(), "Bash(git:*)".to_string()],
        );

        let merged = merge_permission_rules(base, overlay);

        assert!(!merged
            .allow
            .iter()
            .any(|item| item.eq_ignore_ascii_case("Bash(git:*)")));
        assert!(!merged
            .ask
            .iter()
            .any(|item| item.eq_ignore_ascii_case("Write")));
        assert!(merged.deny.iter().any(|item| item == "write"));
        assert!(merged.deny.iter().any(|item| item == "Bash(git:*)"));
        assert!(merged.ask.iter().any(|item| item == "read"));
    }

    #[test]
    fn hook_definitions_accept_inline_hooks_block() {
        let overlay = RunConfigOverlay::from_value(serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [{
                    "hooks": [{"type": "prompt", "prompt": "check $ARGUMENTS"}]
                }]
            }
        }));

        let defs = overlay.hook_definitions().unwrap().unwrap();

        assert_eq!(defs.hooks.len(), 1);
        assert!(defs.hooks.contains_key("UserPromptSubmit"));
    }

    fn layer(
        path: &str,
        precedence: u16,
        value: serde_json::Value,
    ) -> deepagent_context::ConfigLayer {
        deepagent_context::ConfigLayer {
            path: path.to_string(),
            precedence,
            value,
        }
    }

    #[test]
    fn layered_rules_union_across_scopes_with_deny_over_ask_over_allow() {
        let overlay = RunConfigOverlay::from_layers(vec![
            layer(
                "user",
                210,
                serde_json::json!({"permissions": {"allow": ["Read"], "ask": ["Bash(npm:*)"]}}),
            ),
            layer(
                "project",
                310,
                serde_json::json!({"permissions": {"allow": ["Bash(npm:*)"], "deny": ["Read"]}}),
            ),
        ]);

        let merged = overlay
            .merged_permission_rules(PermissionRules::default())
            .unwrap();

        // Sets union across scopes instead of the project block replacing the
        // user block; deny > ask > allow wins regardless of source.
        assert!(merged.deny.iter().any(|r| r == "Read"));
        assert!(!merged.allow.iter().any(|r| r == "Read"));
        assert!(merged.ask.iter().any(|r| r == "Bash(npm:*)"));
        assert!(!merged.allow.iter().any(|r| r == "Bash(npm:*)"));
    }

    #[test]
    fn managed_rules_are_supreme_over_lower_scopes() {
        let overlay = RunConfigOverlay::from_layers(vec![
            layer(
                "project",
                310,
                serde_json::json!({"permissions": {"deny": ["WebSearch"], "allow": ["Bash(rm:*)"]}}),
            ),
            layer(
                "managed",
                600,
                serde_json::json!({"permissions": {"allow": ["WebSearch"], "deny": ["Bash(rm:*)"]}}),
            ),
        ]);

        let merged = overlay
            .merged_permission_rules(PermissionRules::default())
            .unwrap();

        // Managed allow strips the project deny; managed deny beats the
        // project allow.
        assert!(merged.allow.iter().any(|r| r == "WebSearch"));
        assert!(!merged.deny.iter().any(|r| r == "WebSearch"));
        assert!(merged.deny.iter().any(|r| r == "Bash(rm:*)"));
        assert!(!merged.allow.iter().any(|r| r == "Bash(rm:*)"));
    }

    #[test]
    fn model_override_reads_trimmed_scalar_via_precedence_chain() {
        let overlay =
            RunConfigOverlay::from_value(serde_json::json!({"model": "  deepseek-reasoner "}));
        assert_eq!(overlay.model_override(), Some("deepseek-reasoner"));
        let empty = RunConfigOverlay::from_value(serde_json::json!({"model": "   "}));
        assert_eq!(empty.model_override(), None);
        let none = RunConfigOverlay::from_value(serde_json::json!({}));
        assert_eq!(none.model_override(), None);
    }
}
