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
    let mut assembler = ContextAssembler::new(root)
        .push(
            ContextSourceKind::System,
            "deepagent.system_prompt",
            u16::MAX,
            true,
            true,
            format!(
                "{base}{boundary}",
                base = crate::system_prompt::system_prompt_base(),
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

    assembler = assembler.push(
        ContextSourceKind::PermissionContext,
        "deepagent.permissions",
        950,
        false,
        true,
        crate::permissions_prompt::sandbox_instructions(sandbox_mode),
    );

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
    build_system_manifest(root, SandboxMode::WorkspaceWrite, None, None, Vec::new()).render()
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
}
