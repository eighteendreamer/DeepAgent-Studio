//! Skill-catalog reminder injection — channel A of the auto-activation
//! design (`.kiro/specs/skill-marketplace/design.md` §Auto-Activation §通道 A).
//!
//! At the start of every chat turn the runtime asks
//! [`SkillCatalogSendState::next_delta`] for the catalog block to attach to
//! the system prompt. The first call against a fresh state renders the full
//! visible registry (every skill not carrying
//! `disable-model-invocation: true`); subsequent calls only render skills
//! whose ids are NEW since the last call (Property 11: send-once per session,
//! plus deltas when the registry changes).
//!
//! Callers that mutate the registry (`reload_skills`, install / uninstall,
//! marketplace install) clear the per-session state via
//! [`ChatService::reset_sent_skills`][crate::chat_service::ChatService::reset_sent_skills]
//! / [`reset_all_sent_skills`][crate::chat_service::ChatService::reset_all_sent_skills]
//! so the next turn re-announces the changed entries.
//!
//! Master-disable rules (R5.8 / R10.5):
//! - `settings.skill_catalog_enabled == false` → return `None` unconditionally.
//! - `settings.skill_catalog_char_budget == 0` → treated as disabled.
//!
//! Empty registry (R5.9): if no visible skills are registered (or every
//! visible skill is `disable_model_invocation = true`), nothing is injected.
//!
//! Delta-rendering strategy:
//! On a fresh state the first call renders the full visible registry through
//! [`SkillRegistry::formatted_catalog`][deepagent_skills::SkillRegistry::formatted_catalog]
//! (which honors the budget, BuiltIn-never-truncated, names-only fallback,
//! etc.). Subsequent calls build a delta-only registry by cloning the live
//! one and unregistering every skill that's already been announced; the
//! resulting `<available-skills>` block then lists only the new ids. The
//! per-call budget is the same — the delta is always smaller than the full
//! catalog so no extra accounting is needed (Property 12).

use std::collections::HashSet;

use deepagent_skills::SkillRegistry;

use crate::settings::AppSettings;

/// Per-session send-once tracker for the `<available-skills>` reminder.
///
/// One instance per `(session_id, ChatService)` pair, owned by the chat
/// service's session-keyed map. Mutated each turn by
/// [`SkillCatalogSendState::next_delta`].
#[derive(Debug, Default, Clone)]
pub struct SkillCatalogSendState {
    /// Ids that have already been announced to the model in a prior turn of
    /// this session. Filled lazily as deltas are rendered.
    sent_ids: HashSet<String>,
}

impl SkillCatalogSendState {
    /// Fresh state with no skills marked as sent.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget every previously-sent id so the next [`Self::next_delta`] call
    /// re-announces the full visible registry. Called from
    /// [`ChatService::reset_sent_skills`][crate::chat_service::ChatService::reset_sent_skills]
    /// when the skill set materially changes (install, uninstall, reload,
    /// marketplace install).
    pub fn reset(&mut self) {
        self.sent_ids.clear();
    }

    /// Snapshot the current sent set (read-only). Mostly for tests.
    pub fn sent_count(&self) -> usize {
        self.sent_ids.len()
    }

    /// Compute the catalog reminder block to inject for this turn — or
    /// `None` when no visible skills are available.
    ///
    /// On success returns a fully-formatted `<available-skills>` block
    /// (already including the header + envelope tags rendered by
    /// [`SkillRegistry::formatted_catalog`]); the caller is responsible for
    /// any outer system-reminder wrapping and for splicing it into the
    /// system prompt.
    ///
    /// Mutates `self` to remember which ids have been seen. A fresh session
    /// gets the full visible catalog once; later calls emit only newly visible
    /// skills, or `None` when nothing changed.
    pub fn next_delta(
        &mut self,
        registry: &SkillRegistry,
        settings: &AppSettings,
    ) -> Option<String> {
        // R5.8: master switch off → never inject.
        if !settings.skill_catalog_enabled {
            return None;
        }
        // R10.5: zero budget is a stronger off-switch (also covers a UI
        // slider pinned to the minimum).
        if settings.skill_catalog_char_budget == 0 {
            return None;
        }

        // Visible ids = registry minus disable_model_invocation skills (R5.7).
        // Sorted to keep the rendered listing deterministic across runs;
        // BTreeMap inside SkillRegistry already iterates in id order so this
        // matches `formatted_catalog`'s own ordering.
        let visible_ids: Vec<String> = registry
            .catalog()
            .into_iter()
            .filter(|m| !m.disable_model_invocation)
            .map(|m| m.id.clone())
            .collect();

        // R5.9: empty registry → no reminder.
        if visible_ids.is_empty() {
            return None;
        }

        // Current visible catalog. `sent_ids` is kept for diagnostics/reset
        // compatibility, not for suppressing entries from the model.
        // Render the current catalog. `formatted_catalog` renders every
        // visible entry against the configured budget. Two consequences worth
        // flagging:
        //
        // 1. The current catalog reuses the same budget every turn, mirroring
        //    how tool schemas are repeatedly made available to the model.
        //
        // 2. The BuiltIn-never-truncated invariant from `formatted_catalog`
        //    still applies: if an entry's origin is BuiltIn it keeps
        //    its full description even under tight budgets.
        let new_visible_ids: HashSet<String> = visible_ids
            .iter()
            .filter(|id| !self.sent_ids.contains(id.as_str()))
            .cloned()
            .collect();
        if new_visible_ids.is_empty() {
            return None;
        }

        let mut delta_registry = registry.clone();
        for meta in registry.catalog() {
            if !new_visible_ids.contains(&meta.id) {
                delta_registry.unregister(&meta.id);
            }
        }

        let rendered = delta_registry.formatted_catalog(settings.skill_catalog_char_budget);
        if rendered.is_empty() {
            // Degenerate: budget was non-zero but the renderer returned the
            // empty string anyway (e.g. every entry's `disable_model_
            // invocation` flag flipped between the visible scan above and
            // the render call — racy but possible in pathological tests).
            // Treat as no-op and DON'T mark ids as sent so the next turn can
            // try again.
            return None;
        }

        // Track visible ids for diagnostics and reset-related tests.
        for id in new_visible_ids {
            self.sent_ids.insert(id);
        }

        Some(rendered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionPreset, PermissionPresetVisibility};
    use deepagent_skills::{frontmatter, Skill, SkillOrigin, SkillRegistry};

    /// Build an [`AppSettings`] preconfigured for catalog-on with the default
    /// budget. Tests that want a different budget / disabled state mutate
    /// the returned struct. Mirrors the field shape produced by
    /// `SettingsService::initialize` so any future field added to
    /// [`AppSettings`] surfaces as a compile error here, forcing the test
    /// helper to keep up.
    fn settings_default() -> AppSettings {
        AppSettings {
            catalog: deepagent_models::ModelCatalog::auto_select(
                deepagent_models::DEEPSEEK_BASE_URL.to_string(),
                vec![
                    deepagent_models::ModelInfo {
                        id: "deepseek-v4-flash".into(),
                        object: "model".into(),
                        owned_by: "deepseek".into(),
                        context_window: None,
                        max_output_tokens: None,
                    },
                    deepagent_models::ModelInfo {
                        id: "deepseek-v4-pro".into(),
                        object: "model".into(),
                        owned_by: "deepseek".into(),
                        context_window: None,
                        max_output_tokens: None,
                    },
                ],
            )
            .expect("auto_select with both V4 roles produces a valid catalog"),
            discovered_at: 0,
            approval_policy: crate::settings::ApprovalPolicy::default(),
            sandbox_mode: crate::settings::SandboxMode::default(),
            terminal_shell: crate::settings::TerminalShell::default(),
            permission_rules: deepagent_hooks::PermissionRules::default(),
            hooks_json: String::new(),
            thinking_depth: deepagent_models::ThinkingDepth::default(),
            verification_policy: crate::settings::VerificationPolicy::default(),
            web_search: crate::settings::WebSearchSettings::default(),
            vision: crate::settings::VisionSettings::default(),
            tool_search_mode: crate::settings::SettingsService::DEFAULT_TOOL_SEARCH_MODE,
            tool_search_auto_threshold_chars: None,
            skill_catalog_enabled: true,
            skill_catalog_char_budget: 8000,
            skill_install_ai_review_enabled: true,
            skill_install_ai_review_model: None,
            active_permission_preset: PermissionPreset::default(),
            permission_preset_visibility: PermissionPresetVisibility::default(),
            welcome_name: String::new(),
        }
    }

    /// Build a skill with a description and the requested origin. Triggers
    /// fall out of the description automatically (irrelevant to these tests).
    fn skill(id: &str, name: &str, desc: &str, origin: SkillOrigin) -> Skill {
        let fm = frontmatter::parse(&format!(
            "---\nname: {name}\ndescription: \"{desc}\"\n---\nbody"
        ));
        Skill::from_frontmatter(id, &fm, origin).expect("valid frontmatter")
    }

    /// Build a skill carrying `disable-model-invocation: true`.
    fn hidden_skill(id: &str, name: &str, desc: &str, origin: SkillOrigin) -> Skill {
        let fm = frontmatter::parse(&format!(
            "---\nname: {name}\ndescription: \"{desc}\"\ndisable-model-invocation: true\n---\nbody"
        ));
        Skill::from_frontmatter(id, &fm, origin).expect("valid frontmatter")
    }

    #[test]
    fn first_call_sends_all_visible_skills() {
        // Validates: Requirements R5.1, R5.5.
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "alpha skill", SkillOrigin::User));
        reg.register(skill("b", "B", "bravo skill", SkillOrigin::User));
        reg.register(skill("c", "C", "charlie skill", SkillOrigin::Installed));

        let mut state = SkillCatalogSendState::new();
        let out = state.next_delta(&reg, &settings_default()).unwrap();
        assert!(out.contains("<available-skills>"));
        assert!(out.contains("- a:"), "first turn should announce id 'a'");
        assert!(out.contains("- b:"), "first turn should announce id 'b'");
        assert!(out.contains("- c:"), "first turn should announce id 'c'");
        assert_eq!(state.sent_count(), 3);
    }

    #[test]
    fn second_call_emits_nothing_when_registry_unchanged() {
        // Performance invariant: the model gets the current skill list once,
        // then only receives deltas when the registry changes.
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "alpha skill", SkillOrigin::User));
        reg.register(skill("b", "B", "bravo skill", SkillOrigin::User));

        let mut state = SkillCatalogSendState::new();
        let _first = state.next_delta(&reg, &settings_default()).unwrap();
        let second = state.next_delta(&reg, &settings_default());
        assert!(second.is_none());
        assert_eq!(state.sent_count(), 2);
    }

    #[test]
    fn after_install_announces_only_new_skill() {
        // Newly installed skills appear as a delta; previously announced skills
        // are not repeated into the prompt.
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "alpha skill", SkillOrigin::User));

        let mut state = SkillCatalogSendState::new();
        let first = state.next_delta(&reg, &settings_default()).unwrap();
        assert!(first.contains("- a:"));

        // Install a new skill mid-session.
        reg.register(skill("b", "B", "bravo skill", SkillOrigin::Installed));

        let delta = state.next_delta(&reg, &settings_default()).unwrap();
        assert!(
            !delta.contains("- a:"),
            "previously announced id 'a' should not be repeated"
        );
        assert!(delta.contains("- b:"), "delta should include new id 'b'");
        assert_eq!(state.sent_count(), 2);

        let third = state.next_delta(&reg, &settings_default());
        assert!(third.is_none());
    }

    #[test]
    fn reset_re_announces_full_catalog() {
        // Validates: Requirements R5.6 (reload triggers re-announce).
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "alpha skill", SkillOrigin::User));
        reg.register(skill("b", "B", "bravo skill", SkillOrigin::User));

        let mut state = SkillCatalogSendState::new();
        let _first = state.next_delta(&reg, &settings_default()).unwrap();
        assert_eq!(state.sent_count(), 2);

        state.reset();
        assert_eq!(state.sent_count(), 0);

        let after_reset = state.next_delta(&reg, &settings_default()).unwrap();
        assert!(after_reset.contains("- a:"));
        assert!(after_reset.contains("- b:"));
    }

    #[test]
    fn skips_when_master_switch_disabled() {
        // Validates: Requirements R5.8.
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "alpha", SkillOrigin::User));

        let mut state = SkillCatalogSendState::new();
        let mut settings = settings_default();
        settings.skill_catalog_enabled = false;

        assert!(state.next_delta(&reg, &settings).is_none());
        // And we did NOT mark anything sent — flipping the switch back on
        // should still announce the full catalog.
        assert_eq!(state.sent_count(), 0);
    }

    #[test]
    fn skips_when_budget_is_zero() {
        // Validates: Requirements R10.5.
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "alpha", SkillOrigin::User));

        let mut state = SkillCatalogSendState::new();
        let mut settings = settings_default();
        settings.skill_catalog_char_budget = 0;

        assert!(state.next_delta(&reg, &settings).is_none());
        assert_eq!(state.sent_count(), 0);
    }

    #[test]
    fn skips_when_registry_empty() {
        // Validates: Requirements R5.9.
        let reg = SkillRegistry::new();
        let mut state = SkillCatalogSendState::new();
        assert!(state.next_delta(&reg, &settings_default()).is_none());
    }

    #[test]
    fn filters_disable_model_invocation_skills() {
        // Validates: Requirements R5.7.
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "visible alpha", SkillOrigin::User));
        reg.register(hidden_skill(
            "secret",
            "Secret",
            "user-only skill",
            SkillOrigin::User,
        ));
        reg.register(skill("b", "B", "visible bravo", SkillOrigin::User));

        let mut state = SkillCatalogSendState::new();
        let out = state.next_delta(&reg, &settings_default()).unwrap();
        assert!(out.contains("- a:"));
        assert!(out.contains("- b:"));
        assert!(
            !out.contains("- secret:"),
            "disable_model_invocation skill must not appear in catalog"
        );
        // Hidden id is also NOT marked sent — toggling its flag off later
        // should announce it. This is consistent with `formatted_catalog`'s
        // visibility filter.
        assert_eq!(state.sent_count(), 2);
    }

    #[test]
    fn registry_with_only_hidden_skills_emits_nothing() {
        // Validates: Requirements R5.7, R5.9.
        let mut reg = SkillRegistry::new();
        reg.register(hidden_skill(
            "h1",
            "Hidden 1",
            "user-only",
            SkillOrigin::User,
        ));
        reg.register(hidden_skill(
            "h2",
            "Hidden 2",
            "user-only",
            SkillOrigin::User,
        ));

        let mut state = SkillCatalogSendState::new();
        assert!(state.next_delta(&reg, &settings_default()).is_none());
    }

    #[test]
    fn uninstall_followed_by_reset_re_announces_remaining() {
        // Belt-and-braces: after a skill is uninstalled the per-session state
        // should be reset (the chat service does this via
        // `reset_sent_skills`). Once reset, the next render announces only
        // the surviving ids. This test exercises that flow without going
        // through the chat service.
        let mut reg = SkillRegistry::new();
        reg.register(skill("a", "A", "alpha", SkillOrigin::User));
        reg.register(skill("b", "B", "bravo", SkillOrigin::User));

        let mut state = SkillCatalogSendState::new();
        let _first = state.next_delta(&reg, &settings_default()).unwrap();
        assert_eq!(state.sent_count(), 2);

        // Simulate an uninstall + reset.
        reg.unregister("b");
        state.reset();

        let out = state.next_delta(&reg, &settings_default()).unwrap();
        assert!(out.contains("- a:"));
        assert!(!out.contains("- b:"));
        assert_eq!(state.sent_count(), 1);
    }
}
