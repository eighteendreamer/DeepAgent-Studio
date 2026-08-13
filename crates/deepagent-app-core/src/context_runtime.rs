use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_context::{ContextManifest, ContextPolicy, HeuristicTokenizer};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload};
use deepagent_core::id::SessionId;
use deepagent_core::message::Message;
use deepagent_hooks::{HookContext, HookData, HookOutcome, HookPoint, HookRegistry};
use deepagent_models::{ModelClient, ToolSchema};
use deepagent_runtime::{
    CompactionTrigger, PrefireNote, ReactiveCompaction, ReactiveContextCompactor,
};

use crate::knowledge_service::KnowledgeService;
use crate::plugin_runtime::{PluginOutputStyleEntry, PluginRuntimeProjection};
use crate::settings::{SandboxMode, SettingsService};
use crate::skill_catalog_reminder::SkillCatalogSendState;
use crate::skills_service::SkillsService;
use crate::system_context::{build_context_pack_snapshot, build_system_manifest};
use crate::tool_manifest::{deferred_tools_announcement, ToolManifest};

pub(crate) type RemoteContextFuture = Pin<Box<dyn Future<Output = Result<Option<String>>> + Send>>;
pub(crate) type RemoteContextFactory = Arc<dyn Fn(String) -> RemoteContextFuture + Send + Sync>;

pub(crate) struct HookedReactiveContextCompactor {
    client: Arc<ModelClient>,
    model: String,
    hooks: Arc<HookRegistry>,
    session_id: SessionId,
    breaker: Mutex<CompactionBreaker>,
}

/// Unified debounce + circuit breaker across the run's reactive compactions
/// (Phase E). Prevents overflow→compact→overflow loops from thrashing: a
/// cooldown window between attempts, and a trip after repeated ineffective
/// compactions (token count failed to shrink).
#[derive(Debug, Default)]
struct CompactionBreaker {
    last_attempt: Option<std::time::Instant>,
    ineffective_attempts: u32,
    tripped: bool,
}

impl CompactionBreaker {
    const COOLDOWN: std::time::Duration = std::time::Duration::from_secs(15);
    // Trip after this many consecutive ineffective compactions. Aligned with
    // Claude Code autoCompact.ts::MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES (3).
    const MAX_INEFFECTIVE: u32 = 3;

    fn admit(&mut self) -> bool {
        if self.tripped {
            return false;
        }
        if let Some(last) = self.last_attempt {
            if last.elapsed() < Self::COOLDOWN {
                return false;
            }
        }
        self.last_attempt = Some(std::time::Instant::now());
        true
    }

    fn record_ineffective(&mut self) {
        self.ineffective_attempts += 1;
        if self.ineffective_attempts >= Self::MAX_INEFFECTIVE {
            self.tripped = true;
        }
    }

    fn record_success(&mut self) {
        self.ineffective_attempts = 0;
    }
}

impl HookedReactiveContextCompactor {
    pub(crate) fn new(
        client: Arc<ModelClient>,
        model: String,
        hooks: Arc<HookRegistry>,
        session_id: SessionId,
    ) -> Self {
        Self {
            client,
            model,
            hooks,
            session_id,
            breaker: Mutex::new(CompactionBreaker::default()),
        }
    }

    /// Summarize a compaction `zone` (the older turns being replaced) into a
    /// structured block via the model compactor, appending the working-set
    /// re-injection (modified files, failed checks, invoked skills). Shared by
    /// the synchronous stage-2 compaction and the prefire pass-1.
    async fn summarize_zone(&self, zone: &[Message]) -> String {
        let rendered = zone
            .iter()
            .map(render_message_for_compaction)
            .collect::<Vec<_>>();
        let goal = zone
            .iter()
            .rev()
            .find(|message| message.role == deepagent_core::message::Role::User)
            .map(|message| message.content.clone())
            .unwrap_or_default();
        let summary =
            deepagent_context::ModelCompactor::new(self.client.clone(), self.model.clone())
                .summarize(&goal, &deepagent_context::TaskSummary::default(), &rendered)
                .await;
        let mut summary_block = summary.to_context_block();
        // Post-compaction re-injection (Phase E): modified files, failed checks
        // and invoked skills from the compacted zone survive the summary so the
        // model keeps its working set.
        if let Some(reinjection) = compaction_reinjection_block(zone) {
            summary_block.push_str("\n\n");
            summary_block.push_str(&reinjection);
        }
        summary_block
    }
}

/// Cheap fingerprint of a conversation prefix for prefire NOTE₁ validity
/// (Grok `compaction.rs::fingerprint_prefix`). A mismatch means the prefix
/// changed (edit / rewind / branch) since pass-1, so the cached note no longer
/// summarizes the current prefix and must be dropped.
fn fingerprint_prefix(items: &[Message]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    items.len().hash(&mut hasher);
    for message in items {
        let tag: u8 = match message.role {
            deepagent_core::message::Role::System => 0,
            deepagent_core::message::Role::User => 1,
            deepagent_core::message::Role::Assistant => 2,
            deepagent_core::message::Role::Tool => 3,
        };
        tag.hash(&mut hasher);
        render_message_for_compaction(message).hash(&mut hasher);
    }
    hasher.finish()
}

#[async_trait]
impl ReactiveContextCompactor for HookedReactiveContextCompactor {
    async fn compact(
        &self,
        messages: &[Message],
        trigger: CompactionTrigger,
    ) -> Result<Option<ReactiveCompaction>> {
        const KEEP_RECENT_MESSAGES: usize = 8;

        // Unified debounce / circuit breaker (Phase E): refuse to thrash.
        if !self
            .breaker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .admit()
        {
            return Ok(None);
        }

        let system_end = messages
            .iter()
            .take_while(|message| message.role == deepagent_core::message::Role::System)
            .count();
        let body = &messages[system_end..];
        if body.len() <= KEEP_RECENT_MESSAGES {
            return Ok(None);
        }

        let Some(split) = pairing_safe_compaction_split(body, KEEP_RECENT_MESSAGES) else {
            return Ok(None);
        };

        let before = self
            .hooks
            .dispatch(&HookContext::new(
                self.session_id,
                HookPoint::BeforeCompact,
                HookData::Compact {
                    trigger: trigger.as_str().to_string(),
                    summary: None,
                },
            ))
            .await?;
        if matches!(before, HookOutcome::Deny { .. } | HookOutcome::Ask { .. }) {
            return Ok(None);
        }

        let counter = HeuristicTokenizer::new();
        let count = |items: &[Message]| {
            items
                .iter()
                .map(|message| {
                    deepagent_context::TokenCounter::count(
                        &counter,
                        &render_message_for_compaction(message),
                    )
                })
                .sum::<usize>() as u64
        };
        let tokens_before = count(messages);

        // Stage 1 — micro-compact (Claude parity): clear the BODIES of
        // historical tool results inside the compacted zone while keeping
        // the conversation and tool-call pairing intact. Cheap (no model
        // call) and reversible in spirit — the model can re-run a tool if it
        // really needs the data again. Only when this alone does not free
        // enough space do we pay for a full summary.
        if let Some((micro_messages, cleared)) =
            micro_compact_tool_results(messages, system_end, split)
        {
            let tokens_after = count(&micro_messages);
            // Accept when it frees at least ~20% of the estimate.
            if tokens_after.saturating_mul(10) <= tokens_before.saturating_mul(8) {
                let boundary =
                    format!("[micro-compact] cleared {cleared} historical tool result(s)");
                self.breaker
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .record_success();
                self.hooks
                    .dispatch(&HookContext::new(
                        self.session_id,
                        HookPoint::PostCompact,
                        HookData::Compact {
                            trigger: "micro_compact".to_string(),
                            summary: Some(boundary.clone()),
                        },
                    ))
                    .await?;
                return Ok(Some(ReactiveCompaction {
                    messages: micro_messages,
                    tokens_before,
                    tokens_after,
                    summary: boundary,
                }));
            }
        }

        // Stage 2 — full structured summary of the compacted zone.
        let summary_block = self.summarize_zone(&body[..split]).await;

        let mut compacted = Vec::with_capacity(system_end + 1 + body.len() - split);
        compacted.extend_from_slice(&messages[..system_end]);
        compacted.push(Message::user(format!(
            "[Earlier conversation compacted after context overflow]\n{summary_block}"
        )));
        compacted.extend_from_slice(&body[split..]);

        let tokens_after = count(&compacted);
        if tokens_after >= tokens_before {
            self.breaker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .record_ineffective();
            return Ok(None);
        }
        self.breaker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .record_success();

        self.hooks
            .dispatch(&HookContext::new(
                self.session_id,
                HookPoint::PostCompact,
                HookData::Compact {
                    trigger: trigger.as_str().to_string(),
                    summary: Some(summary_block.clone()),
                },
            ))
            .await?;

        Ok(Some(ReactiveCompaction {
            messages: compacted,
            tokens_before,
            tokens_after,
            summary: summary_block,
        }))
    }

    async fn prefire_pass1(&self, messages: &[Message]) -> Result<Option<PrefireNote>> {
        const KEEP_RECENT_MESSAGES: usize = 8;
        // Background/speculative: no hook dispatch, no breaker mutation — this
        // does not commit any state (Grok prefire pass-1 reads a snapshot only).
        let system_end = messages
            .iter()
            .take_while(|m| m.role == deepagent_core::message::Role::System)
            .count();
        let body = &messages[system_end..];
        if body.len() <= KEEP_RECENT_MESSAGES {
            return Ok(None);
        }
        let Some(split) = pairing_safe_compaction_split(body, KEEP_RECENT_MESSAGES) else {
            return Ok(None);
        };
        let prefix_end = system_end + split;
        let summary_block = self.summarize_zone(&body[..split]).await;
        Ok(Some(PrefireNote {
            note: summary_block,
            prefix_end,
            fingerprint: fingerprint_prefix(&messages[..prefix_end]),
        }))
    }

    async fn apply_prefire(
        &self,
        note: &PrefireNote,
        messages: &[Message],
    ) -> Result<Option<ReactiveCompaction>> {
        // Validity: the cached note only summarizes `messages[..prefix_end]`, so
        // that exact prefix must still be present (Grok fingerprint check;
        // edit/rewind/branch invalidates it).
        if note.prefix_end == 0 || note.prefix_end > messages.len() {
            return Ok(None);
        }
        if fingerprint_prefix(&messages[..note.prefix_end]) != note.fingerprint {
            return Ok(None);
        }
        let system_end = messages
            .iter()
            .take_while(|m| m.role == deepagent_core::message::Role::System)
            .count();
        if system_end > note.prefix_end {
            return Ok(None);
        }
        let before = self
            .hooks
            .dispatch(&HookContext::new(
                self.session_id,
                HookPoint::BeforeCompact,
                HookData::Compact {
                    trigger: CompactionTrigger::AutoCompactThreshold.as_str().to_string(),
                    summary: None,
                },
            ))
            .await?;
        if matches!(before, HookOutcome::Deny { .. } | HookOutcome::Ask { .. }) {
            return Ok(None);
        }

        let counter = HeuristicTokenizer::new();
        let count = |items: &[Message]| {
            items
                .iter()
                .map(|message| {
                    deepagent_context::TokenCounter::count(
                        &counter,
                        &render_message_for_compaction(message),
                    )
                })
                .sum::<usize>() as u64
        };
        let tokens_before = count(messages);

        // Pass-2: system prefix + cached NOTE₁ + verbatim live tail.
        let tail = &messages[note.prefix_end..];
        let mut compacted = Vec::with_capacity(system_end + 1 + tail.len());
        compacted.extend_from_slice(&messages[..system_end]);
        compacted.push(Message::user(format!(
            "[Earlier conversation compacted after context overflow]\n{}",
            note.note
        )));
        compacted.extend_from_slice(tail);

        let tokens_after = count(&compacted);
        if tokens_after >= tokens_before {
            return Ok(None);
        }
        self.hooks
            .dispatch(&HookContext::new(
                self.session_id,
                HookPoint::PostCompact,
                HookData::Compact {
                    trigger: CompactionTrigger::AutoCompactThreshold.as_str().to_string(),
                    summary: Some(note.note.clone()),
                },
            ))
            .await?;
        Ok(Some(ReactiveCompaction {
            messages: compacted,
            tokens_before,
            tokens_after,
            summary: note.note.clone(),
        }))
    }
}

/// Stage-1 micro-compact: replace the bodies of Tool-role messages inside
/// `[system_end, system_end + split)` with a small cleared stub, keeping
/// `tool_call_id` pairing intact. Returns the rewritten message list and the
/// number of cleared results, or `None` when nothing was worth clearing.
fn micro_compact_tool_results(
    messages: &[Message],
    system_end: usize,
    split: usize,
) -> Option<(Vec<Message>, usize)> {
    const MIN_CLEAR_CHARS: usize = 240;
    const CLEARED_STUB: &str = r#"{"status":"cleared","note":"tool result cleared by micro-compact to free context; re-run the tool if this data is needed again"}"#;

    let zone_end = system_end + split;
    let mut cleared = 0usize;
    let mut out = messages.to_vec();
    for message in out.iter_mut().take(zone_end).skip(system_end) {
        if message.role == deepagent_core::message::Role::Tool
            && message.content.chars().count() > MIN_CLEAR_CHARS
        {
            message.content = CLEARED_STUB.to_string();
            cleared += 1;
        }
    }
    (cleared > 0).then_some((out, cleared))
}

/// Tools whose successful calls in the compacted zone identify "files the
/// task already modified".
const REINJECTION_WRITE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "multi_edit",
    "delete_path",
    "move_path",
];

/// Build the post-compaction re-injection block from the zone being
/// summarized: modified files, failed tool checks, and invoked skills. Every
/// list is bounded so the block cannot regrow the context.
fn compaction_reinjection_block(compacted_zone: &[Message]) -> Option<String> {
    const MAX_ITEMS: usize = 8;
    let mut files: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut skills: Vec<String> = Vec::new();
    let mut call_names: HashMap<String, String> = HashMap::new();

    for message in compacted_zone {
        for call in &message.tool_calls {
            call_names.insert(call.id.clone(), call.name.clone());
            if REINJECTION_WRITE_TOOLS.contains(&call.name.as_str()) {
                if let Some(path) = call
                    .arguments
                    .get("path")
                    .or_else(|| call.arguments.get("file_path"))
                    .or_else(|| call.arguments.get("destination"))
                    .and_then(|value| value.as_str())
                {
                    let rendered = path.to_string();
                    if !files.contains(&rendered) && files.len() < MAX_ITEMS {
                        files.push(rendered);
                    }
                }
            }
            if call.name == deepagent_builtins::SKILL_TOOL_NAME {
                if let Some(id) = call.arguments.get("id").and_then(|value| value.as_str()) {
                    let rendered = id.to_string();
                    if !skills.contains(&rendered) && skills.len() < MAX_ITEMS {
                        skills.push(rendered);
                    }
                }
            }
        }
        if message.role == deepagent_core::message::Role::Tool
            && message.content.contains("\"status\":\"error\"")
            && failures.len() < MAX_ITEMS
        {
            let tool = message
                .tool_call_id
                .as_deref()
                .and_then(|id| call_names.get(id))
                .cloned()
                .unwrap_or_else(|| "tool".to_string());
            let preview: String = message.content.chars().take(160).collect();
            failures.push(format!("{tool}: {preview}"));
        }
    }

    if files.is_empty() && failures.is_empty() && skills.is_empty() {
        return None;
    }
    let mut out = String::from("# Working-set carried across compaction\n");
    let mut section = |title: &str, items: &[String]| {
        if !items.is_empty() {
            out.push_str(&format!("\n{title}:\n"));
            for item in items {
                out.push_str(&format!("- {item}\n"));
            }
        }
    };
    section("Files already modified", &files);
    section("Failed checks/tools to remember", &failures);
    section("Skills already invoked", &skills);
    Some(out.trim_end().to_string())
}

pub(crate) struct RunContextRequest<'a> {
    pub(crate) root: &'a Path,
    pub(crate) sandbox_mode: SandboxMode,
    pub(crate) plugin_projection: Option<&'a PluginRuntimeProjection>,
    pub(crate) tool_manifest: &'a ToolManifest,
    pub(crate) skills: Option<&'a Arc<Mutex<SkillsService>>>,
    pub(crate) settings: &'a SettingsService,
    pub(crate) skill_catalog_state: &'a Arc<Mutex<HashMap<String, SkillCatalogSendState>>>,
    pub(crate) session_id: &'a str,
    pub(crate) prior_events: &'a [Event],
    pub(crate) knowledge: Option<&'a Arc<KnowledgeService>>,
    pub(crate) prompt_for_model: &'a str,
    pub(crate) effective_env_mode: Option<&'a str>,
    pub(crate) connection_id: Option<&'a str>,
    pub(crate) remote_context_factory: Option<&'a RemoteContextFactory>,
    pub(crate) context_policy: &'a ContextPolicy,
    pub(crate) history: &'a [Message],
    pub(crate) tools: &'a [ToolSchema],
    pub(crate) context_compacted: bool,
}

pub(crate) struct BuiltRunContext {
    pub(crate) system_manifest: ContextManifest,
    pub(crate) system_prompt: String,
    pub(crate) final_user_prompt: String,
    pub(crate) context_usage: deepagent_context::ContextUsageSnapshot,
}

pub(crate) async fn build_run_context(request: RunContextRequest<'_>) -> Result<BuiltRunContext> {
    // Built-in output style (§7.1): stable per-session block injected into the
    // cacheable system prefix. Read from the persisted setting (env-overridable).
    let output_style_block =
        crate::system_context::output_style_prompt_block(request.settings.output_style());
    let plugin_output_style_block = request
        .plugin_projection
        .and_then(|projection| plugin_output_styles_prompt(&projection.output_styles));
    let tool_catalog_block =
        deferred_tools_announcement(&request.tool_manifest.undiscovered_deferred_names);
    let skill_catalog_blocks = build_skill_catalog_blocks(
        request.skills,
        request.settings,
        request.skill_catalog_state,
        request.session_id,
        request.prior_events,
    )?;

    let system_manifest = build_system_manifest(
        request.root,
        request.sandbox_mode,
        output_style_block,
        plugin_output_style_block,
        tool_catalog_block,
        skill_catalog_blocks,
    );
    let system_prompt = system_manifest.render();

    let knowledge_reminder = request
        .knowledge
        .map(|k| k.passive_block(request.prompt_for_model))
        .filter(|b| !b.trim().is_empty())
        .map(|b| crate::system_reminder::wrap(&b));
    let remote_reminder = build_remote_reminder(
        request.effective_env_mode,
        request.connection_id,
        request.remote_context_factory,
    )
    .await;

    let final_user_prompt = compose_prompt_with_runtime_reminders(
        request.prompt_for_model,
        [&remote_reminder, &knowledge_reminder],
    );
    let context_usage = build_context_pack_snapshot(
        request.context_policy,
        &system_prompt,
        request.history,
        &final_user_prompt,
        request.tools,
        request.context_compacted,
        &HeuristicTokenizer::new(),
    );

    Ok(BuiltRunContext {
        system_manifest,
        system_prompt,
        final_user_prompt,
        context_usage,
    })
}

fn build_skill_catalog_blocks(
    skills: Option<&Arc<Mutex<SkillsService>>>,
    settings: &SettingsService,
    skill_catalog_state: &Arc<Mutex<HashMap<String, SkillCatalogSendState>>>,
    session_id: &str,
    prior_events: &[Event],
) -> Result<Vec<String>> {
    let mut blocks = Vec::new();
    let Some(skills) = skills else {
        return Ok(blocks);
    };

    let settings = settings.load().ok().flatten();
    let catalog_block = if let Some(settings) = settings {
        let svc = skills
            .lock()
            .map_err(|_| CoreError::invalid("skills service mutex poisoned"))?;
        let mut state_map = skill_catalog_state.lock().unwrap_or_else(|p| {
            let mut inner = p.into_inner();
            inner.clear();
            inner
        });
        let entry = state_map.entry(session_id.to_string()).or_default();
        entry.next_delta(svc.manager().registry(), &settings)
    } else {
        None
    };

    if let Some(block) = catalog_block {
        blocks.push(crate::system_reminder::wrap(&block));
    }
    let invoked = collect_invoked_skill_records_from_events(prior_events);
    if let Some(block) = invoked_skills_reminder(&invoked) {
        blocks.push(crate::system_reminder::wrap(&block));
    }
    Ok(blocks)
}

async fn build_remote_reminder(
    effective_env_mode: Option<&str>,
    connection_id: Option<&str>,
    remote_context_factory: Option<&RemoteContextFactory>,
) -> Option<String> {
    if !matches!(effective_env_mode, Some("remote")) {
        return None;
    }
    let (Some(factory), Some(conn_id)) = (remote_context_factory, connection_id) else {
        return None;
    };
    match factory(conn_id.to_string()).await {
        Ok(Some(block)) if !block.trim().is_empty() => Some(crate::system_reminder::wrap(&block)),
        Ok(_) => None,
        Err(err) => {
            tracing::warn!(connection_id = conn_id, error = %err, "failed to collect remote context");
            None
        }
    }
}

fn compose_prompt_with_runtime_reminders<'a>(
    prompt: &str,
    reminders: impl IntoIterator<Item = &'a Option<String>>,
) -> String {
    let prefixes = reminders
        .into_iter()
        .filter_map(|reminder| reminder.as_ref())
        .cloned()
        .collect::<Vec<_>>();
    if prefixes.is_empty() {
        prompt.to_string()
    } else {
        format!("{}\n\n{}", prefixes.join("\n\n"), prompt)
    }
}

pub(crate) fn pairing_safe_compaction_split(
    messages: &[Message],
    keep_recent: usize,
) -> Option<usize> {
    if messages.len() <= keep_recent {
        return None;
    }
    let mut split = messages.len() - keep_recent;
    // Never start the retained tail with tool results. Walk back through the
    // complete result group to retain its requesting assistant turn.
    while split > 0 && messages[split].role == deepagent_core::message::Role::Tool {
        split -= 1;
    }
    (split > 0).then_some(split)
}

pub(crate) fn render_message_for_compaction(message: &Message) -> String {
    let mut rendered = format!("{:?}: {}", message.role, message.content);
    if let Some(reasoning) = message.reasoning_content.as_deref() {
        rendered.push_str("\nreasoning: ");
        rendered.push_str(reasoning);
    }
    for call in &message.tool_calls {
        rendered.push_str(&format!(
            "\ntool_use id={} name={} arguments={}",
            call.id, call.name, call.arguments
        ));
    }
    if let Some(call_id) = message.tool_call_id.as_deref() {
        rendered.push_str("\ntool_result_for: ");
        rendered.push_str(call_id);
    }
    rendered
}

pub(crate) fn plugin_output_styles_prompt(styles: &[PluginOutputStyleEntry]) -> Option<String> {
    let styles = styles
        .iter()
        .filter(|style| !style.prompt.trim().is_empty())
        .collect::<Vec<_>>();
    if styles.is_empty() {
        return None;
    }

    if let Some(style) = styles
        .iter()
        .find(|style| style.force_for_plugin.unwrap_or(false))
    {
        let mut out = format!(
            "# Plugin output style\nThe enabled plugin output style `{}` from plugin `{}` is forced for this run. Follow it for user-visible responses unless it conflicts with safety, tool-use rules, or the user's explicit request.\n\n<output-style name=\"{}\">\n{}\n</output-style>",
            style.name,
            style.plugin_name,
            style.name,
            truncate_prompt_block(&style.prompt, 6000),
        );
        let forced_count = styles
            .iter()
            .filter(|style| style.force_for_plugin.unwrap_or(false))
            .count();
        if forced_count > 1 {
            out.push_str(&format!(
                "\n\nNote: {forced_count} plugin output styles are forced; using the first loaded style."
            ));
        }
        return Some(out);
    }

    let mut out = String::from(
        "# Plugin output styles\nEnabled plugins provide these optional output styles. Use one only when the user explicitly asks for it by exact name or the active plugin workflow clearly calls for it. These style prompts do not override safety, tool-use rules, or the user's explicit request.",
    );
    for style in styles.iter().take(8) {
        out.push_str(&format!(
            "\n\n## `{}`\nPlugin: `{}`\nDescription: {}\n\n<output-style name=\"{}\">\n{}\n</output-style>",
            style.name,
            style.plugin_name,
            single_line(&style.description),
            style.name,
            truncate_prompt_block(&style.prompt, 2500),
        ));
    }
    if styles.len() > 8 {
        out.push_str(&format!(
            "\n\n{} additional plugin output style(s) are available through the plugin runtime but omitted from this prompt block.",
            styles.len() - 8
        ));
    }
    Some(out)
}

fn truncate_prompt_block(input: &str, cap: usize) -> String {
    if input.chars().count() <= cap {
        return input.trim().to_string();
    }
    let keep = cap.saturating_sub(32);
    let mut out = input.trim().chars().take(keep).collect::<String>();
    out.push_str("\n\n[truncated]");
    out
}

fn single_line(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvokedSkillRecord {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) body: String,
    pub(crate) base_dir: Option<String>,
    pub(crate) resources: Vec<String>,
}

pub(crate) fn collect_invoked_skill_ids_from_events(
    events: &[Event],
) -> std::collections::HashSet<String> {
    let mut pending: HashMap<String, String> = HashMap::new();
    let mut invoked = std::collections::HashSet::new();
    for event in events {
        match &event.payload {
            EventPayload::ToolCallRequested { call }
                if call.name == deepagent_builtins::SKILL_TOOL_NAME =>
            {
                if let Some(id) = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    pending.insert(call.id.clone(), id.to_string());
                }
            }
            EventPayload::ToolCallCompleted { call_id, ok, .. } if *ok => {
                if let Some(id) = pending.remove(call_id) {
                    invoked.insert(id);
                }
            }
            _ => {}
        }
    }
    invoked
}

pub(crate) fn collect_invoked_skill_records_from_events(
    events: &[Event],
) -> Vec<InvokedSkillRecord> {
    let mut pending: HashMap<String, String> = HashMap::new();
    let mut index_by_id: HashMap<String, usize> = HashMap::new();
    let mut records = Vec::new();

    for event in events {
        match &event.payload {
            EventPayload::ToolCallRequested { call }
                if call.name == deepagent_builtins::SKILL_TOOL_NAME =>
            {
                if let Some(id) = call
                    .arguments
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    pending.insert(call.id.clone(), id.to_string());
                }
            }
            EventPayload::ToolCallCompleted {
                call_id,
                ok,
                output,
                ..
            } if *ok => {
                let Some(requested_id) = pending.remove(call_id) else {
                    continue;
                };
                let Some(record) = invoked_skill_record_from_output(&requested_id, output) else {
                    continue;
                };
                if let Some(index) = index_by_id.get(&record.id).copied() {
                    records[index] = record;
                } else {
                    index_by_id.insert(record.id.clone(), records.len());
                    records.push(record);
                }
            }
            _ => {}
        }
    }

    records
}

fn invoked_skill_record_from_output(
    requested_id: &str,
    output: &serde_json::Value,
) -> Option<InvokedSkillRecord> {
    let body = output
        .get("body")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let id = output
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(requested_id)
        .to_string();
    let name = output
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&id)
        .to_string();
    let base_dir = output
        .get("base_dir")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let resources = output
        .get("resources")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Some(InvokedSkillRecord {
        id,
        name,
        body,
        base_dir,
        resources,
    })
}

pub(crate) fn invoked_skills_reminder(records: &[InvokedSkillRecord]) -> Option<String> {
    if records.is_empty() {
        return None;
    }

    let mut out = String::from(
        "The following skills were invoked earlier in this session; their instructions are \
reproduced below for reference. Treat them as guidance for output quality and formatting, NOT as \
mandates about tooling: if a skill's suggested approach repeatedly fails in this environment \
(missing dependencies, failing installs, incompatible tooling), abandon that approach and use the \
platform's built-in tools or another dependency-free route instead — completing the user's goal \
always outranks following a skill. Do not re-invoke a listed skill unless you need fresh arguments \
or updated resources.\n\n<invoked-skills>\n",
    );
    for record in records {
        out.push_str("\n### Skill: ");
        out.push_str(&record.name);
        if record.id != record.name {
            out.push_str(" (`");
            out.push_str(&record.id);
            out.push_str("`)");
        }
        out.push('\n');
        if let Some(base_dir) = &record.base_dir {
            out.push_str("Base directory: ");
            out.push_str(base_dir);
            out.push('\n');
        }
        if !record.resources.is_empty() {
            out.push_str("Resources:\n");
            for resource in &record.resources {
                out.push_str("- ");
                out.push_str(resource);
                out.push('\n');
            }
        }
        out.push('\n');
        out.push_str(&record.body);
        out.push('\n');
    }
    out.push_str("\n</invoked-skills>");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_call(
        id: &str,
        name: &str,
        args: serde_json::Value,
    ) -> deepagent_core::message::ToolCall {
        deepagent_core::message::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    fn assistant_with_calls(calls: Vec<deepagent_core::message::ToolCall>) -> Message {
        Message {
            role: deepagent_core::message::Role::Assistant,
            content: String::new(),
            reasoning_content: None,
            tool_calls: calls,
            tool_call_id: None,
        }
    }

    #[test]
    fn micro_compact_clears_only_old_large_tool_results() {
        let big = "x".repeat(1_000);
        let messages = vec![
            Message::system("sys"),
            Message::user("task"),
            assistant_with_calls(vec![tool_call("c1", "read_file", serde_json::json!({}))]),
            Message::tool_result("c1", big.clone()),
            Message::assistant("progress"),
            Message::tool_result("c2", big.clone()), // inside retained tail
        ];
        // system_end=1, split=3 -> zone is indices [1..4)
        let (out, cleared) = micro_compact_tool_results(&messages, 1, 3).unwrap();
        assert_eq!(cleared, 1);
        assert!(out[3].content.contains("cleared by micro-compact"));
        assert_eq!(out[3].tool_call_id.as_deref(), Some("c1"));
        // Retained-tail tool result untouched.
        assert_eq!(out[5].content, big);
        // Small/short results and non-tool roles untouched.
        assert_eq!(out[1].content, "task");
    }

    #[test]
    fn micro_compact_returns_none_when_nothing_to_clear() {
        let messages = vec![
            Message::system("sys"),
            Message::user("task"),
            Message::assistant("answer"),
        ];
        assert!(micro_compact_tool_results(&messages, 1, 2).is_none());
    }

    #[test]
    fn reinjection_block_carries_files_failures_and_skills() {
        let zone = vec![
            assistant_with_calls(vec![
                tool_call(
                    "c1",
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs"}),
                ),
                tool_call("c2", "skill", serde_json::json!({"id": "docx"})),
                tool_call("c3", "bash", serde_json::json!({"command": "cargo test"})),
            ]),
            Message::tool_result("c1", r#"{"status":"ok"}"#.to_string()),
            Message::tool_result(
                "c3",
                r#"{"status":"error","error":"2 tests failed"}"#.to_string(),
            ),
        ];
        let block = compaction_reinjection_block(&zone).unwrap();
        assert!(block.contains("src/lib.rs"));
        assert!(block.contains("docx"));
        assert!(block.contains("bash"));
        assert!(block.contains("2 tests failed"));
    }

    #[test]
    fn reinjection_block_empty_zone_returns_none() {
        assert!(compaction_reinjection_block(&[Message::user("hello")]).is_none());
    }

    #[test]
    fn compaction_breaker_cooldown_and_trip() {
        let mut breaker = CompactionBreaker::default();
        assert!(breaker.admit());
        // Immediately after: cooldown rejects.
        assert!(!breaker.admit());
        breaker.record_ineffective();
        breaker.record_ineffective();
        // Two ineffective attempts: not yet tripped (CC threshold is 3).
        breaker.last_attempt = None;
        assert!(breaker.admit());
        breaker.record_ineffective();
        // Tripped after the third: rejected regardless of time.
        breaker.last_attempt = None;
        assert!(!breaker.admit());
    }

    fn style_entry(
        name: &str,
        description: &str,
        prompt: &str,
        force_for_plugin: Option<bool>,
    ) -> PluginOutputStyleEntry {
        PluginOutputStyleEntry {
            plugin_id: "writer@personal".to_string(),
            plugin_name: "writer".to_string(),
            name: name.to_string(),
            description: description.to_string(),
            prompt: prompt.to_string(),
            force_for_plugin,
            source_path: None,
        }
    }

    #[test]
    fn plugin_output_styles_prompt_uses_forced_style() {
        let block = plugin_output_styles_prompt(&[
            style_entry(
                "writer:plain",
                "Plain style",
                "Use plain language.",
                Some(false),
            ),
            style_entry(
                "writer:release",
                "Release style",
                "Write crisp release notes.",
                Some(true),
            ),
        ])
        .unwrap();

        assert!(block.contains("writer:release"));
        assert!(block.contains("forced for this run"));
        assert!(block.contains("Write crisp release notes."));
        assert!(!block.contains("Use plain language."));
    }

    #[test]
    fn plugin_output_styles_prompt_lists_optional_styles() {
        let block = plugin_output_styles_prompt(&[style_entry(
            "writer:plain",
            "Plain style\nwith whitespace",
            "Use plain language.",
            None,
        )])
        .unwrap();

        assert!(block.contains("# Plugin output styles"));
        assert!(block.contains("`writer:plain`"));
        assert!(block.contains("Plain style with whitespace"));
        assert!(block.contains("Use plain language."));
    }

    #[test]
    fn compose_prompt_keeps_runtime_reminders_before_user_text() {
        let remote = Some("<system-reminder>\nremote\n</system-reminder>".to_string());
        let knowledge = Some("<system-reminder>\nknowledge\n</system-reminder>".to_string());
        let composed = compose_prompt_with_runtime_reminders("delete temp", [&remote, &knowledge]);

        assert!(composed.starts_with("<system-reminder>\nremote"));
        assert!(composed.contains("</system-reminder>\n\n<system-reminder>\nknowledge"));
        assert!(composed.ends_with("\n\ndelete temp"));
    }

    #[test]
    fn prefire_fingerprint_stable_and_change_sensitive() {
        // Grok parity: fingerprint is stable for an identical prefix and shifts
        // when the prefix content or length changes (edit / rewind / branch).
        let base = vec![
            Message::system("sys"),
            Message::user("first task"),
            Message::assistant("did first"),
        ];
        assert_eq!(fingerprint_prefix(&base), fingerprint_prefix(&base));

        let edited = vec![
            Message::system("sys"),
            Message::user("FIRST task changed"),
            Message::assistant("did first"),
        ];
        assert_ne!(
            fingerprint_prefix(&base),
            fingerprint_prefix(&edited),
            "an edited prefix must invalidate the cached NOTE₁ fingerprint"
        );

        let longer = {
            let mut v = base.clone();
            v.push(Message::user("second task"));
            v
        };
        assert_ne!(fingerprint_prefix(&base), fingerprint_prefix(&longer));
    }

    /// Real-model end-to-end (no mock): proves proactive auto-compact fires
    /// against a live DeepSeek call and the run continues. Reads the key from
    /// `DEEPSEEK_API_KEY` or the desktop keychain; skips cleanly if absent.
    /// Run with: `cargo test -p deepagent-app-core --features web,runtimes,keychain
    /// -- --ignored real_deepseek_proactive_compaction`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_proactive_compaction_fires_and_run_continues() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_hooks::HookRegistry;
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};
        use deepagent_runtime::{
            Agent, AgentDecision, ModelAgent, ReactiveContextCompactor, RuntimeEvent,
            RuntimeEventSink,
        };
        use std::sync::Mutex as StdMutex;

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

        #[derive(Default)]
        struct CollectSink(StdMutex<Vec<RuntimeEvent>>);
        impl RuntimeEventSink for CollectSink {
            fn emit(&self, event: RuntimeEvent) {
                self.0.lock().unwrap().push(event);
            }
        }
        let sink = Arc::new(CollectSink::default());

        let transport = Arc::new(ReqwestTransport::new());
        let client = Arc::new(ModelClient::new(transport, ModelConfig::deepseek(key)));
        let model = "deepseek-chat";
        let compactor: Arc<dyn ReactiveContextCompactor> =
            Arc::new(HookedReactiveContextCompactor::new(
                client.clone(),
                model.to_string(),
                Arc::new(HookRegistry::new()),
                SessionId::new(),
            ));

        // Long seeded history so the heuristic estimate is well above the low
        // proactive threshold, forcing a pre-request compaction on step 0.
        let history: Vec<Message> = (0..40)
            .map(|i| {
                if i % 2 == 0 {
                    Message::user(format!(
                        "Earlier step {i}: investigate module {i} and report findings with file \
                         paths and rationale in detail."
                    ))
                } else {
                    Message::assistant(format!(
                        "Step {i}: inspected the module, found several functions, recorded the \
                         relevant details for later."
                    ))
                }
            })
            .collect();

        let mut agent = ModelAgent::new(
            client,
            model,
            "You are a helpful engineering assistant.",
            "Summarize the current status in one sentence.",
            vec![],
        )
        .with_history(history)
        .with_reactive_compactor(compactor)
        .with_proactive_compaction(200)
        .with_events(sink.clone());

        let decision = agent
            .think(0, &[])
            .await
            .expect("real DeepSeek think should succeed after proactive compaction");

        let events = sink.0.lock().unwrap();
        let compacted = events.iter().any(|event| {
            matches!(
                event,
                RuntimeEvent::ContextCompacted { strategy, .. }
                    if strategy == "proactive_threshold" || strategy == "prefire_pass2"
            )
        });
        assert!(
            compacted,
            "expected a proactive ContextCompacted event among {} runtime events",
            events.len()
        );
        assert!(matches!(
            decision,
            AgentDecision::Complete(_)
                | AgentDecision::CompleteMessage(_)
                | AgentDecision::CallTool(_)
                | AgentDecision::CallTools(_)
        ));
        eprintln!("[real-model] proactive compaction fired and the run continued OK");
    }

    /// Real-model end-to-end (no mock): validates the DeepSeek contract the
    /// max-tokens recovery logic depends on — a tiny `max_tokens` yields
    /// `finish_reason = Length`, and appending the partial output plus the
    /// continue prompt resumes the answer. Reads the key from
    /// `DEEPSEEK_API_KEY` or the desktop keychain; skips cleanly if absent.
    /// Run with: `cargo test -p deepagent-app-core --features web,runtimes,keychain
    /// -- --ignored real_deepseek_max_tokens --nocapture`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_max_tokens_truncation_then_continue_resumes() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_models::chat::{FinishReason, ResponseRequest};
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};

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

        let client = ModelClient::new(
            Arc::new(ReqwestTransport::new()),
            ModelConfig::deepseek(key),
        );
        let model = "deepseek-chat";
        let system = Message::system("You are a helpful assistant. Answer directly.");
        let user = Message::user(
            "List the numbers from 1 to 40, one per line, as \"N. <english word>\" \
             (e.g. \"1. one\"). Output only the list.",
        );

        // Tiny max_tokens forces truncation → finish_reason = Length.
        let truncated = client
            .stream_response(
                ResponseRequest::new(model.to_string(), vec![system.clone(), user.clone()])
                    .with_max_output_tokens(48),
            )
            .await
            .expect("first (truncated) call");
        let truncated_projection = truncated.assistant_message_projection();
        eprintln!(
            "[real-model] first finish_reason={:?}, content_len={}",
            truncated.finish_reason,
            truncated_projection.content.len()
        );
        assert_eq!(
            truncated.finish_reason,
            Some(FinishReason::Length),
            "tiny max_tokens must truncate the answer"
        );
        assert!(!truncated_projection.content.trim().is_empty());

        // Continuation: partial output + the exact recovery prompt the runtime
        // injects, at a larger budget — the model must resume, not restart.
        let mut partial = Message::assistant(&truncated_projection.content);
        partial.reasoning_content = truncated_projection.reasoning_content.clone();
        let resumed = client
            .stream_response(
                ResponseRequest::new(
                    model.to_string(),
                    vec![
                        system,
                        user,
                        partial,
                        Message::user(
                            "Output token limit hit. Resume directly — no apology, no recap of \
                             what you were doing. Pick up mid-thought if that is where the cut \
                             happened. Break remaining work into smaller pieces.",
                        ),
                    ],
                )
                .with_max_output_tokens(512),
            )
            .await
            .expect("continuation call");
        let resumed_text = resumed.output_text_projection();
        eprintln!(
            "[real-model] continuation finish_reason={:?}, content_len={}",
            resumed.finish_reason,
            resumed_text.len()
        );
        // The continuation produced further output (the run resumed rather than
        // dead-ending on the truncation).
        assert!(
            !resumed_text.trim().is_empty(),
            "continuation must produce further output"
        );
        eprintln!("[real-model] max-tokens truncation + continue resumed OK");
    }

    /// Real-model end-to-end (no mock): a seeded knowledge entry is
    /// background-prefetched and injected into a live DeepSeek run as a
    /// `relevant_memories` reminder, and a `RelevantMemoriesInjected` event
    /// fires (§3.2 acceptance: non-blocking prefetch + visible in run_events).
    /// Run with: `cargo test -p deepagent-app-core --features
    /// web,runtimes,keychain -- --ignored real_deepseek_relevant_memory --nocapture`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_relevant_memory_prefetch_injects_and_emits_event() {
        use crate::knowledge_service::{
            KnowledgeDraftDto, KnowledgeMemoryProvider, KnowledgeService,
        };
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};
        use deepagent_runtime::{Agent, ModelAgent, RuntimeEvent, RuntimeEventSink};
        use std::sync::Mutex as StdMutex;

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

        // Seed a knowledge entry relevant to the run's goal.
        let tmp = tempfile::tempdir().unwrap();
        let knowledge = Arc::new(
            KnowledgeService::open(&tmp.path().join("proj"), &tmp.path().join("glob")).unwrap(),
        );
        knowledge
            .save(KnowledgeDraftDto {
                title: "Windows sandbox stdout relay fix".to_string(),
                body: "Sandboxie Start.exe does not relay sandboxed stdout. Use the workspace \
                       redirect readback fix; never revert it, or the model loses tool output."
                    .to_string(),
                kind: Some("pitfall".to_string()),
                tags: vec!["sandbox".into()],
                scope: Some("project".to_string()),
                source_session: None,
            })
            .unwrap();

        #[derive(Default)]
        struct CollectSink(StdMutex<Vec<RuntimeEvent>>);
        impl RuntimeEventSink for CollectSink {
            fn emit(&self, event: RuntimeEvent) {
                self.0.lock().unwrap().push(event);
            }
        }
        let sink = Arc::new(CollectSink::default());

        let client = Arc::new(ModelClient::new(
            Arc::new(ReqwestTransport::new()),
            ModelConfig::deepseek(key),
        ));
        let provider = Arc::new(KnowledgeMemoryProvider::new(knowledge));
        let mut agent = ModelAgent::new(
            client,
            "deepseek-chat",
            "You are a helpful engineering assistant.",
            "My Sandboxie sandboxed command shows no stdout output on Windows — what is the fix?",
            vec![],
        )
        .with_relevant_memory_provider(provider)
        .with_events(sink.clone());

        // Turn 0: schedules the background prefetch, hits live DeepSeek.
        let _ = agent.think(0, &[]).await.expect("first real turn");
        // Let the (offline, fast) retrieval settle before the next turn polls it.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // Turn 1: collects the settled prefetch and injects it before the request.
        let _ = agent.think(1, &[]).await.expect("second real turn");

        let injected = agent
            .conversation()
            .iter()
            .any(|m| m.content.contains("相关记忆") && m.content.contains("Sandboxie"));
        assert!(
            injected,
            "the seeded memory must be injected into the live run's context"
        );
        let events = sink.0.lock().unwrap();
        assert!(
            events.iter().any(
                |e| matches!(e, RuntimeEvent::RelevantMemoriesInjected { count, .. } if *count > 0)
            ),
            "a RelevantMemoriesInjected event must fire (run_events visibility)"
        );
        eprintln!("[real-model] relevant-memory prefetch injected + event emitted OK");
    }

    /// Real-model end-to-end (no mock): after a stretch of todo inactivity the
    /// periodic todo reminder (§3.1) surfaces in a *live* DeepSeek run and the
    /// run keeps working with it in context. Run with: `cargo test -p
    /// deepagent-app-core --features web,runtimes,keychain -- --ignored
    /// real_deepseek_todo_reminder --nocapture`.
    #[cfg(feature = "keychain")]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_periodic_todo_reminder_surfaces_in_live_run() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use crate::todo_snapshot_reminder::TodoReminderAdapter;
        use deepagent_builtins::{TodoItem, TodoStatus, TodoStore};
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};
        use deepagent_runtime::{Agent, ModelAgent};

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

        // Seed a todo list so the reminder carries the current plan.
        let store = TodoStore::new();
        store.replace(vec![
            TodoItem {
                content: "Investigate the flaky test".to_string(),
                status: TodoStatus::InProgress,
                active_form: "Investigating the flaky test".to_string(),
            },
            TodoItem {
                content: "Write the regression test".to_string(),
                status: TodoStatus::Pending,
                active_form: "Writing the regression test".to_string(),
            },
        ]);

        let client = Arc::new(ModelClient::new(
            Arc::new(ReqwestTransport::new()),
            ModelConfig::deepseek(key),
        ));
        let mut agent = ModelAgent::new(
            client,
            "deepseek-chat",
            "You are a concise engineering assistant. Answer in one short sentence.",
            "Briefly acknowledge you are working on the debugging task.",
            vec![],
        )
        .with_todo_reminder_source(Arc::new(TodoReminderAdapter::new(store)));

        // Drive live turns until the periodic reminder fires (bounded). The
        // model never calls todo_write here (no tools), so the inactivity
        // counter climbs each turn until the threshold.
        let mut injected = false;
        for step in 0..12 {
            let _ = agent.think(step, &[]).await.expect("live turn");
            if agent
                .conversation()
                .iter()
                .any(|m| m.content.contains("todo-tracking tool hasn't been used"))
            {
                injected = true;
                break;
            }
        }
        assert!(
            injected,
            "the periodic todo reminder must surface within the inactivity window in a live run"
        );
        // The reminder carried the seeded plan (pending item renders by
        // content; the in-progress item renders by its active form).
        assert!(agent
            .conversation()
            .iter()
            .any(|m| m.content.contains("Write the regression test")));
        eprintln!("[real-model] periodic todo reminder surfaced in a live run OK");
    }
}
