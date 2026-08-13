//! End-to-end integration test: catalog reminder + `SkillTool` driven by a
//! scripted (mock) LLM client.
//!
//! This stitches together every piece the spec's three-channel
//! auto-activation pipeline relies on (`design.md` §Auto-Activation):
//!
//! - **Channel A** (target) — [`SkillCatalogSendState::next_delta`] turning
//!   a live [`SkillRegistry`][deepagent_skills::SkillRegistry] into the
//!   `<available-skills>` reminder injected into the system prompt
//!   (`crates/deepagent-app-core/src/skill_catalog_reminder.rs` — the
//!   chat-service consumer is in `chat_service.rs::run_in_session`).
//! - **Channel B** (target) — [`SkillTool`] resolving a model-issued
//!   `skill({id, args?})` call into a [`SkillToolOutput`][deepagent_skills::SkillToolOutput]
//!   carrying the disclosed body, base_dir, and resource paths.
//! - **Catalog source** — [`SkillsService::open_v2`] over a real four-tier
//!   [`SkillsRoots`] layout on disk (built-in / user / marketplace /
//!   workspace), exactly the wiring the desktop shell does at startup
//!   (`apps/desktop/src-tauri/src/lib.rs`).
//! - **Mid-session install** — [`SkillsService::install_from_temp`]
//!   simulating a marketplace install landing a new skill into
//!   `<home>/.deepagent/skills/marketplace/`.
//!
//! The "mock LLM client" is a scripted [`MockLlmClient`] type that — for
//! each turn — records the system-prompt block produced by
//! [`SkillCatalogSendState::next_delta`] and returns a canned model
//! response: either `tool_call: skill({id})` (driving [`SkillTool::invoke`])
//! or plain content. This mirrors what a real DeepSeek transport would
//! emit, without forcing the test to script SSE chunks for the runtime
//! engine. The real chat-service `run_in_session` loop is exercised
//! turn-by-turn through unit tests in
//! `chat_service.rs::tests::with_skills_*`; here we validate the
//! cross-component integration the chat-service relies on.
//!
//! _Validates: Requirements R5.1, R5.5, R5.6, R5.7, R6.1, R6.2, R6.4._

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deepagent_app_core::settings::{
    AppSettings, PermissionPreset, PermissionPresetVisibility, SettingsService,
};
use deepagent_app_core::skill_catalog_reminder::SkillCatalogSendState;
use deepagent_app_core::skills_service::SkillsService;
use deepagent_builtins::{SkillTool, SKILL_TOOL_NAME};
use deepagent_models::{ModelCatalog, ModelInfo, DEEPSEEK_BASE_URL};
use deepagent_skills::{SkillRegistry, SkillsRoots};
use deepagent_tools::Tool;

// ---------------------------------------------------------------------------
// Minimal SKILL.md fixtures.
// ---------------------------------------------------------------------------

/// A built-in skill: model-invocable, lives on disk under
/// `<home>/builtin/<id>/SKILL.md`. Body uses the `${ARGS}` placeholder so
/// the [`SkillTool`] substitution path is exercised in the happy case.
const BUILTIN_ALPHA_MD: &str = "---\n\
name: built-in-alpha\n\
description: Built-in alpha skill — perform alpha work. Use to \"do alpha tasks\".\n\
version: 1.0.0\n\
---\n\
# Alpha\n\n\
Alpha body. Args: ${ARGS}.\n";

/// A user-tier skill: model-invocable, lives under `<home>/user/<id>/`.
const USER_BRAVO_MD: &str = "---\n\
name: user-bravo\n\
description: User bravo skill — perform bravo work. Use to \"do bravo tasks\".\n\
version: 1.0.0\n\
---\n\
# Bravo\n\n\
Bravo body.\n";

/// A user-only skill carrying `disable-model-invocation: true`. The catalog
/// reminder MUST omit it (R5.7) and the [`SkillTool`] MUST refuse to
/// disclose its body when invoked by the model (R6.4).
const USER_SECRET_MD: &str = "---\n\
name: user-secret\n\
description: User-only secret skill. Use to \"do secret things\".\n\
disable-model-invocation: true\n\
---\n\
# Secret\n\n\
Should not reach the model body channel.\n";

/// A new skill installed mid-session via [`SkillsService::install_from_temp`].
const MARKETPLACE_CHARLIE_MD: &str = "---\n\
name: marketplace-charlie\n\
description: Marketplace charlie skill — perform charlie work. Use to \"do charlie tasks\".\n\
version: 0.2.0\n\
---\n\
# Charlie\n\n\
Charlie body installed mid-session.\n";

// ---------------------------------------------------------------------------
// Disk fixtures: build the four-tier `SkillsRoots` layout under one tempdir.
// ---------------------------------------------------------------------------

/// Write a SKILL.md fixture into `<root>/<id>/SKILL.md`, creating parent
/// directories as needed.
fn write_skill_dir(root: &Path, id: &str, contents: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(dir.join("SKILL.md"), contents).expect("write SKILL.md");
    dir
}

/// A `~/.deepagent/skills/` layout under a fresh tempdir. Mirrors the four
/// tiers the desktop shell wires up at runtime: `builtin` (read-only,
/// app-resource-style), `user` (top-level under the home dir),
/// `marketplace` (nested under `user`, so the loader's
/// `discover_recursive_excluding` keeps a marketplace skill from being
/// re-classified as `User`), and an optional `workspace` (left `None` here
/// — workspace-tier skills are out of scope for this test).
struct FakeRoots {
    _home: tempfile::TempDir,
    builtin: PathBuf,
    user: PathBuf,
    marketplace: PathBuf,
}

impl FakeRoots {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("create fake home tempdir");
        let builtin = home.path().join("builtin");
        let user = home.path().join(".deepagent").join("skills");
        let marketplace = user.join("marketplace");
        std::fs::create_dir_all(&builtin).expect("create builtin root");
        std::fs::create_dir_all(&user).expect("create user root");
        std::fs::create_dir_all(&marketplace).expect("create marketplace root");
        Self {
            _home: home,
            builtin,
            user,
            marketplace,
        }
    }

    /// Project the on-disk layout into a [`SkillsRoots`] the same way
    /// `apps/desktop/src-tauri/src/lib.rs` does at startup.
    fn as_skills_roots(&self) -> SkillsRoots {
        SkillsRoots {
            builtin: self.builtin.clone(),
            user: self.user.clone(),
            marketplace: self.marketplace.clone(),
            workspace: None,
        }
    }
}

// ---------------------------------------------------------------------------
// AppSettings test factory.
// ---------------------------------------------------------------------------

/// Build a default-shaped [`AppSettings`] suitable for driving the catalog
/// reminder. Mirrors `skill_catalog_reminder::tests::settings_default` but
/// kept here so changes to [`AppSettings`] surface as compile errors in
/// this integration test too.
fn settings_default() -> AppSettings {
    AppSettings {
        catalog: ModelCatalog::auto_select(
            DEEPSEEK_BASE_URL.to_string(),
            vec![
                ModelInfo {
                    id: "deepseek-v4-flash".into(),
                    object: "model".into(),
                    owned_by: "deepseek".into(),
                    context_window: None,
                    max_output_tokens: None,
                },
                ModelInfo {
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
        approval_policy: Default::default(),
        sandbox_mode: Default::default(),
        terminal_shell: Default::default(),
        permission_rules: Default::default(),
        hooks_json: String::new(),
        thinking_depth: Default::default(),
        responses: Default::default(),
        verification_policy: Default::default(),
        web_search: Default::default(),
        vision: Default::default(),
        tool_search_mode: SettingsService::DEFAULT_TOOL_SEARCH_MODE,
        tool_search_auto_threshold_chars: None,
        skill_catalog_enabled: true,
        skill_catalog_char_budget: 8000,
        skill_install_ai_review_enabled: true,
        skill_install_ai_review_model: None,
        active_permission_preset: PermissionPreset::default(),
        permission_preset_visibility: PermissionPresetVisibility::default(),
        welcome_name: String::new(),
        autocompact_reserve_tokens: None,
        output_style: deepagent_app_core::OutputStyle::default(),
        execution_features: deepagent_app_core::ExecutionFeatures::default(),
    }
}

// ---------------------------------------------------------------------------
// Scripted "mock LLM" client.
// ---------------------------------------------------------------------------

/// One scripted assistant response: either a tool call against the `skill`
/// tool, or plain content (terminating the turn). The real DeepSeek
/// transport emits these as Responses SSE events carrying semantic
/// `function_call` output items; here we pre-shape them so the test stays
/// focused on the catalog + tool wiring.
#[derive(Debug, Clone)]
enum CannedAssistant {
    /// Model called the `skill` tool with the given JSON arguments.
    SkillCall { args: serde_json::Value },
    /// Model emitted a final content message and ended the turn.
    Content,
}

/// A tiny scripted LLM client. Each call to [`MockLlmClient::take_turn`]
/// pops the next canned response off the script. The recorded
/// `system_prompt` parameter is what the chat-service would have sent in
/// that turn's `messages[0].content`; this is what we assert against to
/// verify the catalog reminder is (or isn't) present.
#[derive(Debug)]
struct MockLlmClient {
    script: Vec<CannedAssistant>,
    /// `(turn_index, system_prompt_observed)`. The system prompt is the
    /// full string the chat-service would put into the first message of
    /// the request body — i.e. base prompt + dynamic boundary + reminders.
    observed: Vec<(usize, String)>,
}

impl MockLlmClient {
    fn new(script: Vec<CannedAssistant>) -> Self {
        Self {
            script,
            observed: Vec::new(),
        }
    }

    /// Drive one turn. Records the inbound system prompt and pops the next
    /// canned response. Panics if the script is exhausted — the test is
    /// expected to size the script to the number of turns it drives.
    fn take_turn(&mut self, system_prompt: &str) -> CannedAssistant {
        let idx = self.observed.len();
        self.observed.push((idx, system_prompt.to_string()));
        self.script
            .remove(0)
            .clone_marker(/* keep clippy happy */)
    }

    /// Read the system prompt observed at turn `idx`. Panics if `idx` is
    /// out of range.
    fn system_prompt_at(&self, idx: usize) -> &str {
        &self
            .observed
            .iter()
            .find(|(i, _)| *i == idx)
            .expect("turn idx in range")
            .1
    }
}

impl CannedAssistant {
    /// `Vec::remove` already gives us an owned value — this is just a
    /// no-op that keeps clippy from grumbling about an explicit clone in
    /// the call site above.
    fn clone_marker(self) -> Self {
        self
    }
}

/// Render the system prompt for a turn the way `ChatService::run_in_session`
/// does: a fixed "base prompt" sentinel + the catalog reminder block (when
/// the per-session state hands one over). We don't replicate the entire
/// `build_system_prompt(...)` machinery — just the slot where the catalog
/// reminder is spliced in (`chat_service.rs` near `register_skill_catalog
/// _into`-equivalent code), because that's the only piece this test
/// asserts against.
fn render_turn_system_prompt(
    state: &mut SkillCatalogSendState,
    registry: &SkillRegistry,
    settings: &AppSettings,
) -> String {
    let mut prompt = String::from("[BASE-PROMPT]\n\n<<<DYNAMIC>>>\n\n[ENVIRONMENT]");
    if let Some(reminder) = state.next_delta(registry, settings) {
        prompt.push_str("\n\n");
        prompt.push_str(&deepagent_app_core::system_reminder::wrap(&reminder));
    }
    prompt
}

/// Snapshot the live registry the way `ChatService::maybe_register_skill_tool`
/// does: clone it once and wrap it in an `Arc` so the [`SkillTool`] holds
/// an immutable view for the duration of the turn.
fn snapshot_skill_tool(svc: &SkillsService) -> SkillTool {
    SkillTool::new(Arc::new(svc.manager().registry().clone()))
}

// ---------------------------------------------------------------------------
// E2E test.
// ---------------------------------------------------------------------------

/// Drive a multi-turn skill activation flow against a real on-disk
/// `SkillsRoots` layout, asserting all five contracts the task requires:
///
/// 1. turn-0 system message contains `<available-skills>` listing every
///    visible registered id;
/// 2. when the model responds with `tool_call: skill({id: "X"})` the
///    [`SkillTool`] returns the body + base_dir for that id;
/// 3. turn-1 system message does NOT re-announce already-sent ids
///    (send-once delta semantics);
/// 4. installing a new skill mid-session causes the next turn's reminder
///    to carry just the new id as a delta;
/// 5. a skill carrying `disable-model-invocation: true` is invisible to
///    both the catalog reminder AND the skill tool channel.
///
/// _Validates: Requirements R5.1, R5.5, R5.6, R5.7, R6.1, R6.2, R6.4._
#[tokio::test]
async fn end_to_end_skill_activation_through_catalog_and_tool() {
    // ---- Fixture setup --------------------------------------------------
    let roots = FakeRoots::new();
    write_skill_dir(&roots.builtin, "built-in-alpha", BUILTIN_ALPHA_MD);
    write_skill_dir(&roots.user, "user-bravo", USER_BRAVO_MD);
    write_skill_dir(&roots.user, "user-secret", USER_SECRET_MD);

    let mut svc = SkillsService::open_v2(roots.as_skills_roots())
        .expect("SkillsService should open over the fake four-tier layout");
    let settings = settings_default();
    let mut state = SkillCatalogSendState::new();

    // The mock LLM script: turn-0 calls skill(built-in-alpha); turn-1
    // emits content (terminates turn); turn-2 attempts to call the
    // user-only skill (must be rejected); turn-3 emits content after the
    // marketplace install; turn-4 emits content with a fresh state
    // (re-announce path).
    let mut llm = MockLlmClient::new(vec![
        CannedAssistant::SkillCall {
            args: serde_json::json!({ "id": "built-in-alpha", "args": "from-test" }),
        },
        CannedAssistant::Content,
        CannedAssistant::SkillCall {
            args: serde_json::json!({ "id": "user-secret" }),
        },
        CannedAssistant::Content,
        CannedAssistant::Content,
    ]);

    // ---- Sanity: SkillsService loaded all three on-disk skills ----------
    let listed: Vec<_> = svc.list().into_iter().map(|s| s.id).collect();
    assert!(
        listed.contains(&"built-in-alpha".to_string()),
        "expected built-in-alpha in catalog, got {listed:?}"
    );
    assert!(
        listed.contains(&"user-bravo".to_string()),
        "expected user-bravo in catalog, got {listed:?}"
    );
    assert!(
        listed.contains(&"user-secret".to_string()),
        "expected user-secret in catalog (still listed for the UI even though \
         it's hidden from the model), got {listed:?}"
    );

    // ---- Turn 0: the model sees the full visible catalog ----------------
    //
    // Validates R5.1 + R5.7 + R6.1.
    let prompt0 = render_turn_system_prompt(&mut state, svc.manager().registry(), &settings);
    let tool_snapshot_t0 = snapshot_skill_tool(&svc);

    assert!(
        prompt0.contains("<available-skills>"),
        "turn-0 system prompt MUST contain <available-skills> reminder; got:\n{prompt0}"
    );
    assert!(
        prompt0.contains("- built-in-alpha:"),
        "turn-0 reminder MUST list built-in-alpha; got:\n{prompt0}"
    );
    assert!(
        prompt0.contains("- user-bravo:"),
        "turn-0 reminder MUST list user-bravo; got:\n{prompt0}"
    );
    assert!(
        !prompt0.contains("- user-secret:"),
        "turn-0 reminder MUST omit disable-model-invocation skill; got:\n{prompt0}"
    );
    // R6.1: the discovery channel itself must be on every turn — `SkillTool`
    // declares `always_load() == true`, never deferred.
    assert_eq!(
        tool_snapshot_t0.descriptor().name,
        SKILL_TOOL_NAME,
        "tool descriptor must use the reserved skill name"
    );
    assert!(
        tool_snapshot_t0.always_load(),
        "SkillTool must declare always_load=true so tool-search Auto can't hide it"
    );
    assert!(
        !tool_snapshot_t0.should_defer(),
        "SkillTool must not be deferable"
    );

    // The model "responds" to turn-0 with `tool_call: skill(built-in-alpha)`.
    let CannedAssistant::SkillCall { args: t0_args } = llm.take_turn(&prompt0) else {
        panic!("script[0] must be a SkillCall");
    };
    assert_eq!(t0_args["id"], "built-in-alpha");

    // ---- Tool dispatch: SkillTool returns body + base_dir ---------------
    //
    // Validates R6.2 (body resolution + ${ARGS} substitution + base_dir).
    let out0 = tool_snapshot_t0
        .invoke(t0_args.clone())
        .await
        .expect("SkillTool invocation must not error");
    assert!(
        out0.ok,
        "SkillTool invocation should succeed for a registered, visible skill; got {:?}",
        out0
    );
    assert_eq!(out0.value["id"], "built-in-alpha");
    assert_eq!(out0.value["name"], "built-in-alpha");
    let body0 = out0.value["body"]
        .as_str()
        .expect("body must be a string in the tool output");
    assert!(
        body0.contains("Alpha body. Args: from-test."),
        "${{ARGS}} substitution must rewrite to the test arg; body was: {body0:?}"
    );
    let base_dir0 = out0.value["base_dir"]
        .as_str()
        .expect("base_dir must be present for an on-disk skill (R6.2)");
    let base_path = Path::new(base_dir0);
    assert!(
        base_path.is_absolute(),
        "base_dir must be absolute, got {base_dir0}"
    );
    // Compare via canonicalization so Windows long-path prefixes (`\\?\`)
    // don't break a literal `starts_with` against the non-prefixed
    // tempdir handle. Both sides resolve to the same on-disk location.
    let canonical_base =
        std::fs::canonicalize(base_path).expect("base_dir from SkillTool must resolve on disk");
    let canonical_root =
        std::fs::canonicalize(&roots.builtin).expect("fixture builtin root must resolve on disk");
    assert!(
        canonical_base.starts_with(&canonical_root),
        "base_dir must point inside the builtin tier root for built-in-alpha; \
         got {} expected under {}",
        canonical_base.display(),
        canonical_root.display()
    );
    assert!(
        base_path.join("SKILL.md").is_file(),
        "base_dir must contain the on-disk SKILL.md so the model can `read_file` \
         deeper resources; nothing at {}",
        base_path.join("SKILL.md").display()
    );

    // ---- Turn 1: catalog reminder is NOT re-sent ------------------------
    //
    // Validates R5.5 (send-once: the same set of ids must not re-appear in
    // subsequent turns).
    let prompt1 = render_turn_system_prompt(&mut state, svc.manager().registry(), &settings);
    assert!(
        !prompt1.contains("<available-skills>"),
        "turn-1 prompt MUST NOT repeat the unchanged catalog reminder; got:\n{prompt1}"
    );
    let CannedAssistant::Content = llm.take_turn(&prompt1) else {
        panic!("script[1] must be Content (turn-1 finalizes)");
    };

    // ---- Turn 2: tool-channel rejection of disable_model_invocation -----
    //
    // The model can still issue a `skill({id: "user-secret"})` call (it
    // saw the id in the UI somewhere, or guessed) — but the registry's
    // body_for_invoke + the SkillTool's failure mapping must refuse to
    // disclose the body.
    //
    // Validates R6.4.
    let prompt2 = render_turn_system_prompt(&mut state, svc.manager().registry(), &settings);
    let tool_snapshot_t2 = snapshot_skill_tool(&svc);
    let CannedAssistant::SkillCall { args: t2_args } = llm.take_turn(&prompt2) else {
        panic!("script[2] must be a SkillCall (user-secret)");
    };
    assert_eq!(t2_args["id"], "user-secret");

    let out2 = tool_snapshot_t2
        .invoke(t2_args.clone())
        .await
        .expect("SkillTool invocation should not raise an error for a known id");
    assert!(
        !out2.ok,
        "SkillTool MUST reject disable_model_invocation skills; got {:?}",
        out2
    );
    let err = out2.value["error"]
        .as_str()
        .expect("rejection result must include an `error` string");
    assert!(
        err.contains("user-only") || err.contains("disable-model-invocation"),
        "rejection message must hint at user-only nature; got {err:?}"
    );
    // Body must NOT leak through the rejection error.
    assert!(
        !err.contains("Should not reach the model body channel"),
        "rejection message must NOT disclose the skill body; got {err:?}"
    );

    // ---- Mid-session install: marketplace-charlie -----------------------
    //
    // The chat-service consumes a shared `Arc<Mutex<SkillsService>>` and
    // hot-mutates the registry through it. Here we mutate `svc` directly
    // — equivalent to what `skill_market_install` does after the user
    // confirms the install dialog (`apps/desktop/src-tauri/src/lib.rs`
    // task 8 → `skills_service.rs::install_from_temp`).
    let temp_install = tempfile::tempdir().expect("create temp install source");
    let staging = write_skill_dir(
        temp_install.path(),
        "marketplace-charlie",
        MARKETPLACE_CHARLIE_MD,
    );
    let installed_dto = svc
        .install_from_temp(&staging)
        .expect("install_from_temp must land the new skill into marketplace");
    assert_eq!(installed_dto.id, "marketplace-charlie");
    assert_eq!(installed_dto.origin, "installed");
    // R3.6 / R2.3: the new skill must be on disk under marketplace, not
    // under the staging tempdir (which goes away when `temp_install`
    // drops at end of test).
    let installed_dir = roots.marketplace.join("marketplace-charlie");
    assert!(
        installed_dir.join("SKILL.md").is_file(),
        "marketplace install must persist SKILL.md to {}",
        installed_dir.display()
    );

    // ---- Turn 3: delta reminder carries ONLY the new id -----------------
    //
    // Validates R5.6: install → next turn's reminder is the delta (just
    // marketplace-charlie), not a full re-announce.
    let prompt3 = render_turn_system_prompt(&mut state, svc.manager().registry(), &settings);
    assert!(
        prompt3.contains("<available-skills>"),
        "turn-3 prompt MUST carry an updated reminder after install; got:\n{prompt3}"
    );
    assert!(
        prompt3.contains("- marketplace-charlie:"),
        "turn-3 reminder MUST announce the newly-installed marketplace-charlie; got:\n{prompt3}"
    );
    assert!(
        !prompt3.contains("- built-in-alpha:"),
        "turn-3 delta reminder MUST NOT repeat built-in-alpha; got:\n{prompt3}"
    );
    assert!(
        !prompt3.contains("- user-bravo:"),
        "turn-3 delta reminder MUST NOT repeat user-bravo; got:\n{prompt3}"
    );
    assert!(
        !prompt3.contains("- user-secret:"),
        "turn-3 reminder MUST still omit the disable-model-invocation skill; got:\n{prompt3}"
    );
    let CannedAssistant::Content = llm.take_turn(&prompt3) else {
        panic!("script[3] must be Content (post-install turn)");
    };

    // ---- Bonus: explicit reset re-announces every visible id ------------
    //
    // The chat-service's [`ChatService::reset_sent_skills`] / `reset_all_
    // sent_skills` routes (called from `reload_skills`/`uninstall_skill`/
    // `skill_market_install`) clear the per-session state so the next
    // turn re-announces the full visible registry. We exercise that path
    // explicitly to lock in the contract (Property 11 belt-and-braces).
    state.reset();
    let prompt4 = render_turn_system_prompt(&mut state, svc.manager().registry(), &settings);
    assert!(
        prompt4.contains("<available-skills>"),
        "post-reset turn MUST emit a fresh reminder; got:\n{prompt4}"
    );
    for expected in [
        "- built-in-alpha:",
        "- user-bravo:",
        "- marketplace-charlie:",
    ] {
        assert!(
            prompt4.contains(expected),
            "post-reset reminder MUST re-announce '{expected}'; got:\n{prompt4}"
        );
    }
    assert!(
        !prompt4.contains("- user-secret:"),
        "even after reset, disable_model_invocation skill stays out of the reminder; got:\n{prompt4}"
    );
    let CannedAssistant::Content = llm.take_turn(&prompt4) else {
        panic!("script[4] must be Content (post-reset turn)");
    };

    // ---- LLM observation log shape sanity -------------------------------
    //
    // We drove exactly five turns; the observed log should match. This
    // ensures every turn was actually inspected against the script (no
    // dropped assertions).
    assert_eq!(llm.observed.len(), 5, "drove exactly five mock-LLM turns");
    assert_eq!(llm.system_prompt_at(0), &prompt0);
    assert_eq!(llm.system_prompt_at(3), &prompt3);
}

/// Standalone tighter spec for R6.1: with `tool_search_mode = Auto` the
/// `SkillTool` MUST stay loaded — the discovery channel cannot be hidden
/// by the very feature it would otherwise be deferred behind. We assert
/// the trait-level invariants here (`always_load=true`, `should_defer=
/// false`) without spinning up the full `ChatService` since the chat
/// service unit tests
/// (`chat_service::tests::with_skills_registers_skill_tool_in_run_registry`)
/// already cover the registration path under live tool-search mode.
///
/// _Validates: Requirements R6.1._
#[test]
fn skill_tool_is_never_deferred_by_tool_search() {
    let registry = SkillRegistry::new();
    let tool = SkillTool::new(Arc::new(registry));
    assert!(
        tool.always_load(),
        "SkillTool::always_load must be true (R6.1)"
    );
    assert!(
        !tool.should_defer(),
        "SkillTool::should_defer must be false (R6.1)"
    );
}
