use std::path::Path;

use deepagent_context::{
    CacheScope, ContextAssembler, ContextBlock, ContextBlockKind, ContextManifest, ContextPack,
    ContextPolicy, ContextSourceKind, HeuristicTokenizer, TokenCounter,
};
use deepagent_core::message::Message;
use deepagent_models::ToolSchema;

use crate::settings::SandboxMode;

/// Marker separating the stable, prefix-cacheable system prompt from dynamic
/// per-run environment context.
pub const SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "\n\n<<<DYNAMIC>>>\n\n";

/// Build the effective system context manifest for a run: the stable,
/// prefix-cacheable base, then the dynamic environment, permissions, and
/// optional runtime-contributed blocks.
pub(crate) fn build_system_manifest(
    root: &Path,
    sandbox_mode: SandboxMode,
    output_style_block: Option<String>,
    plugin_output_style_block: Option<String>,
    tool_catalog_block: Option<String>,
    skill_catalog_blocks: Vec<String>,
) -> ContextManifest {
    let today = current_date_string();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let env_block = format!(
        "# Environment\n- Today's date: {today}\n- Operating system: {os} ({arch})\n- Working directory: {cwd}\n- When you need current information, use this date — especially the year — in web_search queries.",
        cwd = root.display()
    );
    // The built-in output style sits INSIDE the cacheable static prefix (before
    // the dynamic boundary): it is stable across a session, so keeping it in
    // the prefix preserves the prompt-cache hit (§7.1 cache boundary).
    let style_prefix = output_style_block
        .map(|block| format!("{block}\n\n"))
        .unwrap_or_default();
    let mut assembler = ContextAssembler::new(root)
        .push(
            ContextSourceKind::System,
            "deepagent.system_prompt",
            u16::MAX,
            true,
            true,
            format!(
                "{base}\n\n{style}{boundary}",
                base = crate::system_prompt::system_prompt_base(),
                style = style_prefix,
                boundary = SYSTEM_PROMPT_DYNAMIC_BOUNDARY
            ),
        )
        .push(
            ContextSourceKind::RuntimeEnvironment,
            "deepagent.runtime_environment",
            1000,
            false,
            true,
            env_block,
        );

    if let Some(git) = deepagent_workspace::detect_git_context(root) {
        assembler = assembler.push(
            ContextSourceKind::GitContext,
            "deepagent.git_context",
            900,
            false,
            false,
            git.to_prompt_block(),
        );
    }

    // Project structure (§3.1 `directory` high-value context): a bounded,
    // noise-skipping snapshot of the project layout (type, manifests, README
    // excerpt, truncated file tree) so the model has structural awareness up
    // front instead of spending turns on `list_dir`/`glob`. The scan skips
    // `target`/`node_modules`/dot-dirs and is bounded (depth 4 / 200 entries),
    // so it stays cheap even in large repos. Scan failures are non-fatal
    // (skip the block) — it is context enrichment, never required.
    if let Ok(snapshot) = deepagent_workspace::WorkspaceScanner::default().scan(root) {
        let block = snapshot.to_context_block();
        if !block.trim().is_empty() {
            assembler = assembler.push(
                ContextSourceKind::RuntimeEnvironment,
                "deepagent.workspace_structure",
                880,
                false,
                false,
                block,
            );
        }
    }

    assembler = assembler.push(
        ContextSourceKind::PermissionContext,
        "deepagent.permissions",
        950,
        false,
        true,
        crate::permissions_prompt::sandbox_instructions(sandbox_mode),
    );

    // Project structure (§3.1 `directory` high-value context): a bounded,
    // noise-skipping snapshot of the layout so the model has structural
    // awareness up front instead of spending turns on `list_dir`/`glob`. Skips
    // `target`/`node_modules`/dot-dirs, bounded (depth 4 / 200 entries), so it
    // stays cheap in large repos. Scan failure is non-fatal (skip the block).
    if let Ok(snapshot) = deepagent_workspace::WorkspaceScanner::default().scan(root) {
        let block = snapshot.to_context_block();
        if !block.trim().is_empty() {
            assembler = assembler.push(
                ContextSourceKind::RuntimeEnvironment,
                "deepagent.workspace_structure",
                880,
                false,
                false,
                block,
            );
        }
    }

    if let Some(block) = plugin_output_style_block {
        assembler = assembler.push(
            ContextSourceKind::PluginContext,
            "deepagent.plugin_output_style",
            920,
            false,
            false,
            block,
        );
    }

    if let Some(block) = tool_catalog_block {
        assembler = assembler.push(
            ContextSourceKind::ToolCatalog,
            "deepagent.deferred_tool_catalog",
            890,
            false,
            false,
            block,
        );
    }

    for (index, block) in skill_catalog_blocks.into_iter().enumerate() {
        assembler = assembler.push(
            ContextSourceKind::SkillCatalog,
            format!("deepagent.skill_catalog.{index}"),
            880,
            false,
            false,
            block,
        );
    }

    assembler.assemble(&HeuristicTokenizer::new(), usize::MAX)
}

pub(crate) fn build_context_pack_snapshot(
    policy: &ContextPolicy,
    system_prompt: &str,
    history: &[Message],
    final_user_prompt: &str,
    tools: &[ToolSchema],
    compacted: bool,
    counter: &dyn TokenCounter,
) -> deepagent_context::ContextUsageSnapshot {
    let mut blocks = Vec::new();
    let (stable_prefix, dynamic_runtime) = split_system_prompt_for_context(system_prompt);
    push_context_block(
        &mut blocks,
        ContextBlockKind::StablePrefix,
        "system_prompt_base",
        u8::MAX,
        CacheScope::StablePrefix,
        stable_prefix,
        counter,
    );
    push_context_block(
        &mut blocks,
        ContextBlockKind::DynamicRuntime,
        "runtime_environment",
        200,
        CacheScope::Dynamic,
        dynamic_runtime,
        counter,
    );

    let rendered_history = render_history_for_context_usage(history);
    push_context_block(
        &mut blocks,
        ContextBlockKind::RecentConversation,
        "session_history",
        130,
        CacheScope::Dynamic,
        &rendered_history,
        counter,
    );

    let tool_schema_json = serde_json::to_string(tools).unwrap_or_default();
    push_context_block(
        &mut blocks,
        ContextBlockKind::ToolSchemas,
        "visible_tool_schemas",
        180,
        CacheScope::StablePrefix,
        &tool_schema_json,
        counter,
    );

    push_context_block(
        &mut blocks,
        ContextBlockKind::UserGoal,
        "current_user_prompt",
        u8::MAX,
        CacheScope::Dynamic,
        final_user_prompt,
        counter,
    );

    let pack = ContextPack::new(blocks);
    deepagent_context::ContextUsageSnapshot::from_pack(policy, &pack, compacted)
}

pub(crate) fn split_system_prompt_for_context(system_prompt: &str) -> (&str, &str) {
    if let Some(idx) = system_prompt.find(SYSTEM_PROMPT_DYNAMIC_BOUNDARY) {
        let dynamic_start = idx + SYSTEM_PROMPT_DYNAMIC_BOUNDARY.len();
        (&system_prompt[..idx], &system_prompt[dynamic_start..])
    } else {
        (system_prompt, "")
    }
}

fn render_history_for_context_usage(history: &[Message]) -> String {
    history
        .iter()
        .map(|m| format!("{:?}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_context_block(
    blocks: &mut Vec<ContextBlock>,
    kind: ContextBlockKind,
    source: &str,
    priority: u8,
    cache_scope: CacheScope,
    content: &str,
    counter: &dyn TokenCounter,
) {
    if content.trim().is_empty() {
        return;
    }
    blocks.push(ContextBlock {
        kind,
        source: source.to_string(),
        priority,
        cache_scope,
        estimated_tokens: counter.count(content),
        content: content.to_string(),
    });
}

#[cfg(test)]
pub(crate) fn build_system_prompt(root: &Path) -> String {
    build_system_manifest(
        root,
        SandboxMode::WorkspaceWrite,
        None,
        None,
        None,
        Vec::new(),
    )
    .render()
}

/// The built-in output-style prompt block for `style`, or `None` for
/// [`OutputStyle::Default`] (no injection). Wording is DeepSeek-native
/// (Claude Code output styles §7.1: structure aligned, 措辞以 DeepSeek 为准).
pub(crate) fn output_style_prompt_block(style: crate::settings::OutputStyle) -> Option<String> {
    match style {
        crate::settings::OutputStyle::Default => None,
        crate::settings::OutputStyle::Explanatory => Some(
            "# Output style: Explanatory\n\
             Alongside completing the task, add brief “Insight” notes that explain WHY \
             you chose an approach or what a non-obvious piece of code/decision does. Keep \
             insights short and skimmable; never let them slow delivery or bloat the answer."
                .to_string(),
        ),
        crate::settings::OutputStyle::Learning => Some(
            "# Output style: Learning\n\
             Act as a patient teacher: as you work, explain the concepts, trade-offs, and \
             reasoning behind each step so the user learns from the process. Prefer clear, \
             incremental explanations over terse output, but still finish the task."
                .to_string(),
        ),
    }
}

/// Today's date as `YYYY-MM-DD` (local time, falling back to UTC if the local
/// offset can't be determined). Kept dependency-light via the `time` crate.
pub(crate) fn current_date_string() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    now.format(&fmt)
        .unwrap_or_else(|_| format!("{}", now.year()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_date_is_iso_like() {
        let d = current_date_string();
        assert_eq!(d.len(), 10);
        assert_eq!(&d[4..5], "-");
        assert_eq!(&d[7..8], "-");
    }

    #[test]
    fn output_style_block_default_is_none_others_present() {
        use crate::settings::OutputStyle;
        assert!(output_style_prompt_block(OutputStyle::Default).is_none());
        let explanatory = output_style_prompt_block(OutputStyle::Explanatory).unwrap();
        assert!(explanatory.contains("Explanatory"));
        let learning = output_style_prompt_block(OutputStyle::Learning).unwrap();
        assert!(learning.contains("Learning"));
    }

    #[test]
    fn output_style_stays_in_cacheable_prefix_before_boundary() {
        use std::path::Path;
        let manifest = build_system_manifest(
            Path::new("/work/proj"),
            SandboxMode::WorkspaceWrite,
            output_style_prompt_block(crate::settings::OutputStyle::Explanatory),
            None,
            None,
            Vec::new(),
        );
        let system_entry = manifest
            .entries
            .iter()
            .find(|e| e.origin == "deepagent.system_prompt")
            .expect("system entry present");
        assert!(system_entry.cacheable, "static prefix must stay cacheable");
        let style_at = system_entry
            .content
            .find("Output style: Explanatory")
            .expect("style injected");
        let boundary_at = system_entry
            .content
            .find("<<<DYNAMIC>>>")
            .expect("boundary present");
        assert!(
            style_at < boundary_at,
            "output style must sit before the dynamic boundary (cacheable prefix)"
        );
    }

    #[test]
    fn injects_bounded_project_structure_block() {
        // A real temp project so WorkspaceScanner has something to scan.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        // A noise dir that must NOT appear in the structure block.
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/junk.o"), "").unwrap();

        let manifest = build_system_manifest(
            root,
            SandboxMode::WorkspaceWrite,
            None,
            None,
            None,
            Vec::new(),
        );
        let ws = manifest
            .entries
            .iter()
            .find(|e| e.origin == "deepagent.workspace_structure")
            .expect("workspace structure block present");
        assert!(
            ws.content.contains("# Workspace"),
            "has the workspace header"
        );
        assert!(
            ws.content.contains("main.rs"),
            "file tree lists project files"
        );
        assert!(
            !ws.content.contains("target/"),
            "noise dirs must be skipped from the structure block"
        );
        // Enrichment only: not required, not part of the cacheable prefix.
        assert!(!ws.required);
        assert!(!ws.cacheable);

        // Cache-boundary guard (§7.1): the structure block must render in the
        // DYNAMIC section (after `<<<DYNAMIC>>>`), never in the cacheable
        // static prefix — otherwise a project's changing file tree would bust
        // the prompt-cache hit for the whole stable prefix.
        let rendered = manifest.render();
        let boundary_at = rendered
            .find("<<<DYNAMIC>>>")
            .expect("dynamic boundary present in rendered prompt");
        let ws_at = rendered
            .find("# Workspace")
            .expect("workspace block present in rendered prompt");
        assert!(
            ws_at > boundary_at,
            "project-structure block must sit AFTER the dynamic boundary (not in the cacheable prefix)"
        );
    }

    /// Real-model end-to-end (no mock): proves the injected project-structure
    /// block actually reaches DeepSeek and the model can answer a
    /// layout question it could ONLY know from that block. Reads the key from
    /// `DEEPSEEK_API_KEY` or the desktop keychain; skips cleanly if absent. Run:
    /// `cargo test -p deepagent-app-core --features web,runtimes,keychain --
    /// --ignored real_deepseek_project_structure --nocapture`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_sees_injected_project_structure() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_models::chat::ResponseRequest;
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};
        use std::sync::Arc;

        let key = std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| {
                KeychainStore::new("deepagent-studio")
                    .get("deepseek_api_key")
                    .ok()
                    .flatten()
            });
        let Some(key) = key else {
            eprintln!("[skip] no DeepSeek key in env or keychain");
            return;
        };
        eprintln!("[real-model] key resolved (len={})", key.len());

        // A temp project with a distinctive filename the model can only learn
        // from the injected structure block.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::write(root.join("src/zqx_marker_widget.rs"), "// marker\n").unwrap();

        let system_prompt = build_system_prompt(root);
        assert!(
            system_prompt.contains("zqx_marker_widget.rs"),
            "structure block must carry the marker file into the system prompt"
        );

        let client = Arc::new(ModelClient::new(
            Arc::new(ReqwestTransport::new()),
            ModelConfig::deepseek(key),
        ));
        let request = ResponseRequest::with_instructions_and_user_input(
            "deepseek-chat".to_string(),
            system_prompt,
            "Using ONLY the workspace/project-structure information already in your \
             context (do not call any tools), is there a source file whose name contains \
             `zqx_marker_widget`? Answer strictly `YES` or `NO` on the first line.",
        )
        .with_temperature(0.0)
        .with_max_output_tokens(200);
        let response = client.stream_response(request).await.expect("live call");
        let answer = response.output_text_projection();
        eprintln!("[real-model] answer: {answer}");
        assert!(
            answer.to_ascii_uppercase().contains("YES"),
            "model must confirm the marker file from the injected structure; got: {answer}"
        );
    }
}
