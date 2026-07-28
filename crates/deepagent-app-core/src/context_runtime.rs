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
use deepagent_runtime::{ReactiveCompaction, ReactiveContextCompactor};

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
    const MAX_INEFFECTIVE: u32 = 2;

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
}

#[async_trait]
impl ReactiveContextCompactor for HookedReactiveContextCompactor {
    async fn compact(&self, messages: &[Message]) -> Result<Option<ReactiveCompaction>> {
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
                    trigger: "context_overflow".to_string(),
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
        let rendered = body[..split]
            .iter()
            .map(render_message_for_compaction)
            .collect::<Vec<_>>();
        let goal = body
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
        // Post-compaction re-injection (Phase E): modified files, failed
        // checks and invoked skills from the compacted zone survive the
        // summary so the model keeps its working set. Project rules and the
        // task goal are re-injected structurally (system manifest / summary
        // goal), so they need no extra handling here.
        if let Some(reinjection) = compaction_reinjection_block(&body[..split]) {
            summary_block.push_str("\n\n");
            summary_block.push_str(&reinjection);
        }

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
                    trigger: "context_overflow".to_string(),
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
        // Tripped: rejected regardless of time.
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
}
