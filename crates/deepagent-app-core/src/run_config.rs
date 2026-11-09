use std::collections::HashSet;
use std::path::Path;

use deepagent_context::{ConfigLayer, ConfigScope, ConfigSource, DualConfigLoader};
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

#[derive(Default)]
pub(crate) struct RunHookDefinitions {
    pub(crate) host: HookDefinitions,
    pub(crate) project: HookDefinitions,
}

impl RunConfigOverlay {
    pub(crate) fn load(root: &Path, run_overrides: serde_json::Value) -> Self {
        let overlay = DualConfigLoader::new(root)
            .with_run_overrides(run_overrides)
            .load();
        Self {
            value: overlay.value,
            sources: overlay.sources,
            layers: overlay.layers,
            errors: overlay.errors,
        }
    }

    #[cfg(test)]
    fn from_value(value: serde_json::Value) -> Self {
        Self::from_layers(vec![ConfigLayer {
            path: "run_overrides".into(),
            precedence: deepagent_context::RUN_PRECEDENCE,
            value,
        }])
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
        project_trusted: bool,
    ) -> Result<EffectivePermissionProfile> {
        // Merge precedence is not authority. Project preferences may narrow
        // the user/host ceiling, but only explicit host/run/managed sources
        // may grant more access. Never recover policy from the merged value:
        // doing so loses both origin and cross-field/alias precedence.
        let mut ceiling = profile;
        for layer in &self.layers {
            let scope = layer.scope();
            if scope == ConfigScope::Unknown || (layer.is_project_scoped() && !project_trusted) {
                tracing::info!(source = %layer.path, ?scope,
                    "skipping permission profile from unauthorized config source");
                continue;
            }
            let candidate = Self::apply_profile_value(&layer.value, profile).map_err(|error| {
                CoreError::invalid(format!(
                    "invalid permission config in {}: {error}",
                    layer.path
                ))
            })?;
            profile = match scope {
                ConfigScope::Plugin | ConfigScope::Project | ConfigScope::ProjectLocal => {
                    restrict_profile_to(candidate, ceiling)
                }
                ConfigScope::User => {
                    ceiling = candidate;
                    candidate
                }
                ConfigScope::Run | ConfigScope::Managed => candidate,
                ConfigScope::Unknown => unreachable!("unknown origins are filtered above"),
            };
        }
        Ok(normalize_profile(profile))
    }

    fn apply_profile_value(
        value: &serde_json::Value,
        mut profile: EffectivePermissionProfile,
    ) -> Result<EffectivePermissionProfile> {
        if let Some(permission_profile) =
            string_field(value, &["permission_profile", "permissionProfile"])
        {
            match permission_profile {
                "read_only" => {
                    profile.approval_policy = ApprovalPolicy::AlwaysAsk;
                    profile.sandbox_mode = SandboxMode::ReadOnly;
                }
                "workspace_write" => {
                    profile.approval_policy = ApprovalPolicy::AlwaysAsk;
                    profile.sandbox_mode = SandboxMode::WorkspaceWrite;
                }
                "developer" => {
                    profile = PermissionPreset::AutoReview.to_effective_profile();
                }
                "full_access" => {
                    profile = PermissionPreset::FullAccess.to_effective_profile();
                }
                other => {
                    return Err(CoreError::invalid(format!(
                        "invalid permission profile in project config: {other}"
                    )));
                }
            }
        }
        if let Some(preset) =
            string_field(value, &["permission_preset", "active_permission_preset"])
        {
            if let Some(parsed) = PermissionPreset::parse(preset) {
                profile = parsed.to_effective_profile();
            } else {
                return Err(CoreError::invalid(format!(
                    "invalid permission preset in project config: {preset}"
                )));
            }
        }
        if let Some(policy) =
            parse_overlay_enum::<ApprovalPolicy>(value, &["approval_policy", "approvalPolicy"])?
        {
            profile.approval_policy = policy;
        }
        if let Some(sandbox) =
            parse_overlay_enum::<SandboxMode>(value, &["sandbox_mode", "sandboxMode"])?
        {
            profile.sandbox_mode = sandbox;
        }
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
        let model = string_field(&self.value, &["model"])?.trim();
        // Local import compatibility, not a provider model alias or a claim
        // about Opus/1M capabilities: preserve the old catalog-role default
        // only for recognized aliases from the loader's Claude file tiers.
        // Equal text from a higher run/managed/DeepAgent source is still exact.
        let imported = self
            .layers
            .iter()
            .rev()
            .find(|layer| layer.value.get("model").is_some())
            .is_some_and(|layer| {
                matches!(
                    layer.precedence,
                    deepagent_context::USER_CLAUDE_PRECEDENCE
                        | deepagent_context::PROJECT_CLAUDE_PRECEDENCE
                        | deepagent_context::PROJECT_LOCAL_CLAUDE_PRECEDENCE
                )
            });
        let normalized = model.to_ascii_lowercase();
        let alias = normalized
            .strip_suffix("[1m]")
            .unwrap_or(&normalized)
            .trim();
        if imported && matches!(alias, "sonnet" | "opus" | "haiku" | "opusplan" | "best") {
            tracing::info!("using catalog role default for imported Claude model alias");
            return None;
        }
        Some(model)
    }

    pub(crate) fn provider_override(&self) -> Option<&str> {
        string_field(&self.value, &["provider"])
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn reasoning_effort_override(&self) -> Option<&str> {
        string_field(&self.value, &["reasoning_effort", "reasoningEffort"])
            .filter(|value| !value.trim().is_empty())
    }

    /// Hooks aggregate across the allowed config layers (plugin/user/project/
    /// local/run/managed): every source's handler groups are appended, never
    /// overwritten by a higher scope. Project and project-local layers are
    /// included only after the caller has established project trust; this
    /// keeps a merged scalar value from bypassing the source trust boundary.
    pub(crate) fn hook_definitions(
        &self,
        project_hooks_trusted: bool,
    ) -> Result<RunHookDefinitions> {
        let mut definitions = RunHookDefinitions::default();
        for layer in &self.layers {
            let target = match layer.scope() {
                ConfigScope::Project | ConfigScope::ProjectLocal => {
                    if !project_hooks_trusted {
                        tracing::info!(
                            source = %layer.path,
                            "skipping hooks from untrusted project config layer"
                        );
                        continue;
                    }
                    &mut definitions.project
                }
                ConfigScope::Unknown => {
                    tracing::info!(
                        source = %layer.path,
                        "skipping hooks from config layer with unknown trust origin"
                    );
                    continue;
                }
                _ => &mut definitions.host,
            };
            collect_hook_definitions(&layer.value, target)?;
        }
        Ok(definitions)
    }
}

fn restrict_profile_to(
    mut candidate: EffectivePermissionProfile,
    ceiling: EffectivePermissionProfile,
) -> EffectivePermissionProfile {
    candidate.approval_policy = match (ceiling.approval_policy, candidate.approval_policy) {
        (ApprovalPolicy::AlwaysAsk, _) => ApprovalPolicy::AlwaysAsk,
        (ApprovalPolicy::AutoReview, ApprovalPolicy::FullAccess) => ApprovalPolicy::AutoReview,
        (_, policy) => policy,
    };
    candidate.sandbox_mode = match (ceiling.sandbox_mode, candidate.sandbox_mode) {
        (SandboxMode::ReadOnly, _) => SandboxMode::ReadOnly,
        (SandboxMode::WorkspaceWrite, SandboxMode::FullAccess) => SandboxMode::WorkspaceWrite,
        (_, mode) => mode,
    };
    candidate
}

fn normalize_profile(mut profile: EffectivePermissionProfile) -> EffectivePermissionProfile {
    profile.local_execution_mode = if matches!(
        (profile.approval_policy, profile.sandbox_mode),
        (ApprovalPolicy::FullAccess, SandboxMode::FullAccess)
    ) {
        LocalExecutionMode::Direct
    } else {
        LocalExecutionMode::SandboxiePreferred
    };
    profile.network_always_ask = matches!(profile.approval_policy, ApprovalPolicy::AlwaysAsk);
    profile
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
    use deepagent_context::{
        PROJECT_DEEPAGENT_PRECEDENCE, RUN_PRECEDENCE, USER_DEEPAGENT_PRECEDENCE,
    };

    #[test]
    fn overlay_profile_recomputes_execution_mode_after_overrides() {
        let overlay = RunConfigOverlay::from_value(serde_json::json!({
            "permission_preset": "full_access",
            "approval_policy": "always_ask"
        }));
        let profile = overlay
            .apply_permission_profile(PermissionPreset::Default.to_effective_profile(), false)
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
    fn permission_project_sources_cannot_expand_host_ceiling() {
        use deepagent_context::{
            PROJECT_CLAUDE_PRECEDENCE, PROJECT_LOCAL_CLAUDE_PRECEDENCE,
            PROJECT_LOCAL_DEEPAGENT_PRECEDENCE,
        };
        let payloads = [
            serde_json::json!({"permission_profile":"full_access"}),
            serde_json::json!({"permissionProfile":"full_access"}),
            serde_json::json!({"permission_preset":"full_access"}),
            serde_json::json!({"active_permission_preset":"full_access"}),
            serde_json::json!({"approval_policy":"full_access"}),
            serde_json::json!({"approvalPolicy":"full_access"}),
            serde_json::json!({"sandbox_mode":"full_access"}),
            serde_json::json!({"sandboxMode":"full_access"}),
        ];
        let bases = [
            ApprovalPolicy::AlwaysAsk,
            ApprovalPolicy::AutoReview,
            ApprovalPolicy::FullAccess,
        ]
        .into_iter()
        .flat_map(|policy| {
            [
                SandboxMode::ReadOnly,
                SandboxMode::WorkspaceWrite,
                SandboxMode::FullAccess,
            ]
            .into_iter()
            .map(move |mode| {
                normalize_profile(EffectivePermissionProfile {
                    approval_policy: policy,
                    sandbox_mode: mode,
                    ..PermissionPreset::Default.to_effective_profile()
                })
            })
        })
        .collect::<Vec<_>>();
        for precedence in [
            PROJECT_CLAUDE_PRECEDENCE,
            PROJECT_DEEPAGENT_PRECEDENCE,
            PROJECT_LOCAL_CLAUDE_PRECEDENCE,
            PROJECT_LOCAL_DEEPAGENT_PRECEDENCE,
        ] {
            for payload in &payloads {
                let config = RunConfigOverlay::from_layers(vec![layer(
                    "project",
                    precedence,
                    payload.clone(),
                )]);
                for base in &bases {
                    for trusted in [false, true] {
                        assert_eq!(
                            config.apply_permission_profile(*base, trusted).unwrap(),
                            *base,
                            "source={precedence}, payload={payload}, trusted={trusted}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn permission_project_preferences_need_trust_and_keep_local_precedence() {
        use deepagent_context::PROJECT_LOCAL_DEEPAGENT_PRECEDENCE;
        let config = RunConfigOverlay::from_layers(vec![
            layer(
                "project",
                PROJECT_DEEPAGENT_PRECEDENCE,
                serde_json::json!({"permission_profile":"read_only"}),
            ),
            layer(
                "local",
                PROJECT_LOCAL_DEEPAGENT_PRECEDENCE,
                serde_json::json!({"permission_profile":"workspace_write"}),
            ),
        ]);
        let full = PermissionPreset::FullAccess.to_effective_profile();
        assert_eq!(config.apply_permission_profile(full, false).unwrap(), full);
        assert_eq!(
            config.apply_permission_profile(full, true).unwrap(),
            PermissionPreset::Default.to_effective_profile()
        );
    }

    #[test]
    fn permission_host_run_and_managed_authority_is_source_scoped() {
        use deepagent_context::{MANAGED_PRECEDENCE, PLUGIN_PRECEDENCE};
        let readonly = normalize_profile(EffectivePermissionProfile {
            sandbox_mode: SandboxMode::ReadOnly,
            ..PermissionPreset::Default.to_effective_profile()
        });
        let full = serde_json::json!({"permission_preset":"full_access"});
        for precedence in [
            USER_DEEPAGENT_PRECEDENCE,
            RUN_PRECEDENCE,
            MANAGED_PRECEDENCE,
        ] {
            let config =
                RunConfigOverlay::from_layers(vec![layer("host", precedence, full.clone())]);
            assert_eq!(
                config.apply_permission_profile(readonly, false).unwrap(),
                PermissionPreset::FullAccess.to_effective_profile()
            );
        }
        for precedence in [PLUGIN_PRECEDENCE, 0] {
            let config =
                RunConfigOverlay::from_layers(vec![layer("not a grant", precedence, full.clone())]);
            assert_eq!(
                config.apply_permission_profile(readonly, true).unwrap(),
                readonly
            );
        }
        // A legacy merged value without provenance is not an execution grant.
        let mut unknown = RunConfigOverlay::from_layers(Vec::new());
        unknown.value = full;
        assert_eq!(
            unknown.apply_permission_profile(readonly, true).unwrap(),
            readonly
        );
    }

    #[test]
    fn permission_managed_fields_override_lower_presets_and_aliases() {
        use deepagent_context::MANAGED_PRECEDENCE;
        let config = RunConfigOverlay::from_layers(vec![
            layer(
                "user",
                USER_DEEPAGENT_PRECEDENCE,
                serde_json::json!({
                    "permission_preset":"full_access",
                    "sandbox_mode":"full_access",
                    "approval_policy":"full_access"
                }),
            ),
            layer(
                "project",
                PROJECT_DEEPAGENT_PRECEDENCE,
                serde_json::json!({"permission_profile":"full_access"}),
            ),
            layer(
                "run",
                RUN_PRECEDENCE,
                serde_json::json!({"permissionProfile":"full_access"}),
            ),
            layer(
                "managed",
                MANAGED_PRECEDENCE,
                serde_json::json!({"sandboxMode":"read_only", "approvalPolicy":"always_ask"}),
            ),
        ]);
        let resolved = config
            .apply_permission_profile(PermissionPreset::Default.to_effective_profile(), true)
            .unwrap();
        assert_eq!(resolved.sandbox_mode, SandboxMode::ReadOnly);
        assert_eq!(resolved.approval_policy, ApprovalPolicy::AlwaysAsk);
        assert_eq!(
            resolved.local_execution_mode,
            LocalExecutionMode::SandboxiePreferred
        );
        assert!(resolved.network_always_ask);
        let explicit = RunConfigOverlay::from_layers(vec![
            layer(
                "project",
                PROJECT_DEEPAGENT_PRECEDENCE,
                serde_json::json!({"permission_preset":"default"}),
            ),
            layer(
                "run",
                RUN_PRECEDENCE,
                serde_json::json!({"permission_profile":"full_access"}),
            ),
        ]);
        assert_eq!(
            explicit
                .apply_permission_profile(PermissionPreset::Default.to_effective_profile(), true)
                .unwrap(),
            PermissionPreset::FullAccess.to_effective_profile()
        );
    }

    #[test]
    fn permission_invalid_project_values_are_filtered_before_parsing() {
        let config = RunConfigOverlay::from_layers(vec![layer(
            "untrusted-project",
            PROJECT_DEEPAGENT_PRECEDENCE,
            serde_json::json!({"sandbox_mode":false}),
        )]);
        let base = PermissionPreset::Default.to_effective_profile();
        assert_eq!(config.apply_permission_profile(base, false).unwrap(), base);
        assert!(config
            .apply_permission_profile(base, true)
            .unwrap_err()
            .to_string()
            .contains("untrusted-project"));
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

        let defs = overlay.hook_definitions(true).unwrap().host;

        assert_eq!(defs.hooks.len(), 1);
        assert!(defs.hooks.contains_key("UserPromptSubmit"));
    }

    #[test]
    fn project_hook_layers_are_filtered_until_trust_is_granted() {
        let hook = |prompt: &str| {
            serde_json::json!({
                "hooks": {
                    "SessionStart": [{
                        "hooks": [{"type": "prompt", "prompt": prompt}]
                    }]
                }
            })
        };
        let overlay = RunConfigOverlay::from_layers(vec![
            layer(
                "user/.deepagent/settings.json",
                USER_DEEPAGENT_PRECEDENCE,
                hook("user"),
            ),
            layer(
                ".deepagent/settings.json",
                PROJECT_DEEPAGENT_PRECEDENCE,
                hook("project"),
            ),
            layer("run_overrides", RUN_PRECEDENCE, hook("run")),
        ]);

        let untrusted = overlay.hook_definitions(false).unwrap();
        assert_eq!(
            untrusted
                .host
                .hooks
                .get("SessionStart")
                .map(|groups| groups.len()),
            Some(2),
            "user and explicit run hooks remain, project hook is filtered"
        );
        assert!(untrusted.project.is_empty());

        let trusted = overlay.hook_definitions(true).unwrap();
        assert_eq!(
            trusted
                .project
                .hooks
                .get("SessionStart")
                .map(|groups| groups.len()),
            Some(1)
        );
        assert_eq!(trusted.host.action_count(), 2);
    }

    #[test]
    fn project_hook_scope_matrix_keeps_host_sources_and_skips_unknown() {
        use deepagent_context::{
            MANAGED_PRECEDENCE, PLUGIN_PRECEDENCE, PROJECT_CLAUDE_PRECEDENCE,
            PROJECT_LOCAL_CLAUDE_PRECEDENCE, PROJECT_LOCAL_DEEPAGENT_PRECEDENCE,
            USER_CLAUDE_PRECEDENCE,
        };
        let hook = serde_json::json!({"hooks": {"SessionEnd": [{"hooks": [
            {"type":"command", "command":"fixture"}
        ]}]}});
        let scopes = [
            PLUGIN_PRECEDENCE,
            USER_CLAUDE_PRECEDENCE,
            USER_DEEPAGENT_PRECEDENCE,
            PROJECT_CLAUDE_PRECEDENCE,
            PROJECT_DEEPAGENT_PRECEDENCE,
            PROJECT_LOCAL_CLAUDE_PRECEDENCE,
            PROJECT_LOCAL_DEEPAGENT_PRECEDENCE,
            RUN_PRECEDENCE,
            MANAGED_PRECEDENCE,
            MANAGED_PRECEDENCE + 1,
            u16::MAX,
            0, // Unknown origin must not be promoted to a trusted source.
        ];
        let overlay = RunConfigOverlay::from_layers(
            scopes
                .into_iter()
                .map(|precedence| layer("source", precedence, hook.clone()))
                .collect(),
        );
        for trusted in [false, true] {
            let defs = overlay.hook_definitions(trusted).unwrap();
            assert_eq!(defs.host.action_count(), 7);
            assert_eq!(defs.project.action_count(), if trusted { 4 } else { 0 });
        }
        let untrusted_invalid = RunConfigOverlay::from_layers(vec![
            layer("user", USER_DEEPAGENT_PRECEDENCE, hook),
            layer(
                "project",
                PROJECT_DEEPAGENT_PRECEDENCE,
                serde_json::json!({"hooks_json":"invalid JSON"}),
            ),
        ]);
        assert_eq!(
            untrusted_invalid
                .hook_definitions(false)
                .unwrap()
                .host
                .action_count(),
            1
        );
        assert!(untrusted_invalid.hook_definitions(true).is_err());
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
        assert_eq!(empty.model_override(), Some(""));
        let none = RunConfigOverlay::from_value(serde_json::json!({}));
        assert_eq!(none.model_override(), None);
    }

    #[test]
    fn h89_alias_compatibility_requires_the_winning_claude_source_even_for_equal_values() {
        use deepagent_context::{
            MANAGED_PRECEDENCE, PROJECT_CLAUDE_PRECEDENCE, PROJECT_LOCAL_CLAUDE_PRECEDENCE,
            USER_CLAUDE_PRECEDENCE,
        };
        for precedence in [
            USER_CLAUDE_PRECEDENCE,
            PROJECT_CLAUDE_PRECEDENCE,
            PROJECT_LOCAL_CLAUDE_PRECEDENCE,
        ] {
            for alias in [
                "sonnet",
                "opus",
                "haiku",
                "opusplan",
                "best",
                " Sonnet[1M] ",
            ] {
                let imported = layer(
                    "claude fixture",
                    precedence,
                    serde_json::json!({"model":alias}),
                );
                let overlay = RunConfigOverlay::from_layers(vec![imported.clone()]);
                assert_eq!(overlay.model_override(), None);
                for higher in [RUN_PRECEDENCE, MANAGED_PRECEDENCE] {
                    let overlay = RunConfigOverlay::from_layers(vec![
                        imported.clone(),
                        layer("explicit", higher, serde_json::json!({"model":alias})),
                    ]);
                    assert_eq!(
                        overlay.model_override(),
                        Some(alias.trim()),
                        "identical merged text must not inherit lower-source compatibility"
                    );
                }
            }
        }
        for value in ["", "   ", "future-model", "sonnet"] {
            let overlay = RunConfigOverlay::from_value(serde_json::json!({"model":value}));
            assert_eq!(overlay.model_override(), Some(value.trim()));
        }
    }

    #[test]
    fn provider_and_reasoning_overrides_are_run_scoped_scalars() {
        let overlay = RunConfigOverlay::from_value(serde_json::json!({
            "provider": "  deepseek-official ",
            "reasoningEffort": "high"
        }));

        assert_eq!(overlay.provider_override(), Some("deepseek-official"));
        assert_eq!(overlay.reasoning_effort_override(), Some("high"));
    }
}
