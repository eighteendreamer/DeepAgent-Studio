use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_context::{CompactionKeepWindow, ContextManifest, ContextPolicy, HeuristicTokenizer};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload};
use deepagent_core::id::SessionId;
use deepagent_core::message::Message;
use deepagent_core::response_item::ResponseInputItem;
use deepagent_hooks::{HookContext, HookData, HookOutcome, HookPoint, HookRegistry};
use deepagent_models::{ModelClient, ToolSchema};
use deepagent_persistence::Database;
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

pub(crate) struct SessionSurfaceStore {
    pub db: Arc<Database>,
    pub session_id: SessionId,
}

impl deepagent_runtime::ContextSurfacePersistence for SessionSurfaceStore {
    fn replace_tool_results(
        &self,
        strategy: &str,
        replacements: &[(String, String)],
    ) -> Result<()> {
        if replacements.is_empty() {
            return Ok(());
        }
        let events = deepagent_persistence::event_store::EventStore::new(&self.db)
            .load_session(self.session_id)?;
        let mut patches = Vec::new();
        for (call_id, output) in replacements {
            let source_sequence = events
                .iter()
                .rev()
                .find_map(|event| match &event.payload {
                    EventPayload::ResponseItemAppended {
                        item:
                            ResponseInputItem::FunctionCallOutput { call_id: id, .. }
                            | ResponseInputItem::CustomToolCallOutput { call_id: id, .. },
                    } if id == call_id => Some(event.sequence),
                    EventPayload::ToolCallCompleted { call_id: id, .. } if id == call_id => {
                        Some(event.sequence)
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    CoreError::invalid(format!("cannot replace unpersisted tool output {call_id}"))
                })?;
            patches.push(deepagent_core::event::ToolResultReplacement {
                source_sequence,
                call_id: call_id.clone(),
                replacement_output: output.clone(),
            });
        }
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::recover(&self.db, &clock, self.session_id)?;
        session.append(EventPayload::ContextSurfacePatched {
            version: 1,
            strategy: strategy.to_string(),
            replacements: patches,
        })?;
        Ok(())
    }

    fn record_snip(&self, tags: &[String]) -> Result<()> {
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::recover(&self.db, &clock, self.session_id)?;
        session.append(EventPayload::ContextSnipped {
            tags: tags.to_vec(),
        })?;
        Ok(())
    }
}

pub(crate) struct HookedReactiveContextCompactor {
    client: Arc<ModelClient>,
    model: String,
    hooks: Arc<HookRegistry>,
    session_id: SessionId,
    /// Where this session's durable event log lives. A compaction that is only
    /// adopted in memory is paid for again by the next run, so the boundary has
    /// to be written here, at the moment it is produced.
    db: Arc<Database>,
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
        db: Arc<Database>,
    ) -> Self {
        Self {
            client,
            model,
            hooks,
            session_id,
            db,
            breaker: Mutex::new(CompactionBreaker::default()),
        }
    }

    /// The durable strategy label for a boundary this compactor produced.
    ///
    /// Mirrors what the run loop names the same adoption in its live event
    /// (`model_agent.rs`), so the streamed marker and the replayed timeline row
    /// cannot disagree about what happened.
    fn boundary_strategy(trigger: CompactionTrigger) -> &'static str {
        match trigger {
            CompactionTrigger::AutoCompactThreshold => {
                deepagent_core::compaction_strategy::PROACTIVE_THRESHOLD
            }
            CompactionTrigger::ContextOverflow => {
                deepagent_core::compaction_strategy::REACTIVE_MODEL
            }
        }
    }

    /// Summarize a compaction `zone` (the older turns being replaced) into a
    /// structured block via the model compactor, appending the working-set
    /// re-injection (modified files, failed checks, invoked skills). Shared by
    /// the synchronous stage-2 compaction and the prefire pass-1.
    ///
    /// Returns the block plus what the summary call cost: the run loop never
    /// sees that request, so this is the only place its tokens can be captured.
    async fn summarize_response_item_zone(
        &self,
        zone: &[ResponseInputItem],
    ) -> (String, deepagent_models::Usage) {
        let store = crate::session_memory::SessionMemoryStore::new(self.db.clone());
        if let (Ok(Some(state)), Ok(events)) = (
            store.load(self.session_id),
            deepagent_persistence::event_store::EventStore::new(&self.db)
                .load_session(self.session_id),
        ) {
            if let Some(covered) = state.last_summarized_sequence {
                if !zone.is_empty()
                    && !state.summary.trim().is_empty()
                    && zone.iter().all(|item| {
                        crate::input_runtime::sequence_of_response_item(&events, item)
                            .is_some_and(|seq| seq <= covered)
                    })
                {
                    let summary = crate::input_runtime::with_transcript_pointer(
                        format!(
                            "Session notes maintained during this conversation:\n{}",
                            state.summary
                        ),
                        &self.db.session_transcript_dir(self.session_id),
                    );
                    return (summary, deepagent_models::Usage::default());
                }
            }
        }
        let rendered = zone
            .iter()
            .map(render_response_item_for_compaction)
            .collect::<Vec<_>>();
        let goal = latest_user_content_from_response_items(zone).unwrap_or_default();
        let (summary, usage) =
            deepagent_context::ModelCompactor::new(self.client.clone(), self.model.clone())
                .summarize_accounted(&goal, &deepagent_context::TaskSummary::default(), &rendered)
                .await;
        let mut summary_block = summary.to_context_block();
        // Post-compaction re-injection (Phase E): modified files, failed checks
        // and invoked skills from the compacted zone survive the summary so the
        // model keeps its working set.
        if let Some(reinjection) = compaction_reinjection_block_from_items(zone) {
            summary_block.push_str("\n\n");
            summary_block.push_str(&reinjection);
        }
        (
            crate::input_runtime::with_transcript_pointer(
                summary_block,
                &self.db.session_transcript_dir(self.session_id),
            ),
            usage,
        )
    }

    async fn summarize_zone(&self, zone: &[Message]) -> (String, deepagent_models::Usage) {
        let (_, items) = deepagent_models::response_items_from_messages(zone);
        self.summarize_response_item_zone(&items).await
    }

    fn measure_native_items(&self, instructions: Option<&str>, items: &[ResponseInputItem]) -> u64 {
        deepagent_context::measure_response_surface_for_route(
            instructions,
            items,
            &[],
            |attachment| {
                if deepagent_runtime::AttachmentDataResolver::is_resolvable(
                    &crate::attachment_service::FileAttachmentResolver,
                    attachment,
                ) {
                    self.client.image_token_reserve(&self.model)
                } else {
                    0
                }
            },
        )
        .total() as u64
    }

    fn record_native_boundary(
        &self,
        strategy: &str,
        before: u64,
        after: u64,
        summary: &str,
        tail: &[ResponseInputItem],
    ) -> Result<()> {
        let events = deepagent_persistence::event_store::EventStore::new(&self.db)
            .load_session(self.session_id)?;
        let cut = tail
            .iter()
            .find_map(|item| crate::input_runtime::sequence_of_response_item(&events, item))
            .ok_or_else(|| {
                CoreError::invalid("cannot resolve native compaction tail in durable session log")
            })?;
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::recover(&self.db, &clock, self.session_id)?;
        session.append(EventPayload::ContextCompacted {
            tokens_before: before,
            tokens_after: after,
            strategy: strategy.into(),
            summary: Some(summary.into()),
            first_kept_sequence: Some(cut),
        })?;
        Ok(())
    }

    /// Record the boundary a summarizing pass just produced, so the NEXT run
    /// rebuilds from this summary instead of replaying the transcript this run
    /// already paid to compress (Phase 2 — 边界落盘 covers every compaction path,
    /// not only the assembly-time one).
    ///
    /// Deliberately conservative: with no resolvable cut point (the first history
    /// entry that survived verbatim) nothing is written, because a boundary without
    /// one would also drop the surviving tail from the replayed view, and the
    /// summary only describes what was dropped. The live `ContextCompacted` event
    /// keeps `summary: None` on purpose — runtime logs must not carry prompt
    /// content; the session log already stores model text.
    fn record_durable_boundary(
        &self,
        strategy: &str,
        tokens_before: u64,
        tokens_after: u64,
        summary: &str,
        first_kept: Option<&Message>,
    ) {
        let store = deepagent_persistence::event_store::EventStore::new(&self.db);
        let events = match store.load_session(self.session_id) {
            Ok(events) => events,
            Err(error) => {
                tracing::warn!(%error, "cannot read the session log to place a compaction boundary");
                return;
            }
        };
        let Some(kept) = first_kept else {
            tracing::debug!(
                "compaction kept no identifiable verbatim entry; boundary not recorded"
            );
            return;
        };
        // Matching by content rather than counting records: a batch of parallel
        // tool calls is one history entry but several events.
        let Some(cut) = crate::input_runtime::sequence_of_message(&events, kept) else {
            tracing::debug!(
                "the compaction's first kept entry is not in the log; boundary skipped"
            );
            return;
        };
        let clock = deepagent_core::clock::SystemClock;
        let mut session = match deepagent_session::Session::recover(
            &self.db,
            &clock,
            self.session_id,
        ) {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(%error, "cannot reopen the session to record the compaction boundary");
                return;
            }
        };
        let transcript_dir = session.transcript_dir();
        if let Err(error) = session.append(EventPayload::ContextCompacted {
            tokens_before,
            tokens_after,
            strategy: strategy.to_string(),
            summary: Some(crate::input_runtime::with_transcript_pointer(
                summary.to_string(),
                &transcript_dir,
            )),
            first_kept_sequence: Some(cut),
        }) {
            tracing::warn!(%error, "failed to record the reactive compaction boundary");
        }
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

fn fingerprint_response_item_prefix(items: &[ResponseInputItem]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    items.len().hash(&mut hasher);
    for item in items {
        render_response_item_for_compaction(item).hash(&mut hasher);
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
        let Some(split) = pairing_safe_compaction_split(body) else {
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
                            trigger: deepagent_core::compaction_strategy::MICRO_COMPACT.to_string(),
                            summary: Some(boundary.clone()),
                        },
                    ))
                    .await?;
                return Ok(Some(ReactiveCompaction {
                    messages: micro_messages,
                    response_items: None,
                    tokens_before,
                    tokens_after,
                    summary: boundary,
                    // Micro-compaction drops stale tool results locally; no model
                    // call, so nothing to bill.
                    usage: deepagent_models::Usage::default(),
                }));
            }
        }

        // Stage 2 — full structured summary of the compacted zone.
        let (summary_block, summary_usage) = self.summarize_zone(&body[..split]).await;

        let mut compacted = Vec::with_capacity(system_end + 1 + body.len() - split);
        compacted.extend_from_slice(&messages[..system_end]);
        compacted.push(Message::user(format!(
            "[Earlier conversation compacted to summary]\n{summary_block}"
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

        self.record_durable_boundary(
            Self::boundary_strategy(trigger),
            tokens_before,
            tokens_after,
            &summary_block,
            body.get(split),
        );

        Ok(Some(ReactiveCompaction {
            messages: compacted,
            response_items: None,
            tokens_before,
            tokens_after,
            summary: summary_block,
            usage: summary_usage,
        }))
    }

    async fn compact_response_items(
        &self,
        instructions: Option<&str>,
        items: &[ResponseInputItem],
        trigger: CompactionTrigger,
    ) -> Result<Option<ReactiveCompaction>> {
        if !self
            .breaker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .admit()
        {
            return Ok(None);
        }

        let Some(split) = pairing_safe_response_item_compaction_split(items) else {
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

        let count = |items: &[ResponseInputItem]| self.measure_native_items(instructions, items);
        let tokens_before = count(items);

        let (summary_block, summary_usage) =
            self.summarize_response_item_zone(&items[..split]).await;

        let mut compacted_items = Vec::with_capacity(1 + items.len() - split);
        compacted_items.push(ResponseInputItem::Message {
            role: "user".to_string(),
            content: format!("[Earlier conversation compacted to summary]\n{summary_block}"),
        });
        compacted_items.extend_from_slice(&items[split..]);

        let tokens_after = count(&compacted_items);
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

        // The tail's first entry, projected the way the model sees it, is the cut
        // point the next run must replay verbatim.
        self.record_native_boundary(
            Self::boundary_strategy(trigger),
            tokens_before,
            tokens_after,
            &summary_block,
            &items[split..],
        )?;

        let messages =
            deepagent_models::messages_from_response_items(instructions, &compacted_items);
        Ok(Some(ReactiveCompaction {
            messages,
            response_items: Some(compacted_items),
            tokens_before,
            tokens_after,
            summary: summary_block,
            usage: summary_usage,
        }))
    }

    async fn prefire_pass1(&self, messages: &[Message]) -> Result<Option<PrefireNote>> {
        // Background/speculative: no hook dispatch, no breaker mutation — this
        // does not commit any state (Grok prefire pass-1 reads a snapshot only).
        let system_end = messages
            .iter()
            .take_while(|m| m.role == deepagent_core::message::Role::System)
            .count();
        let body = &messages[system_end..];
        let Some(split) = pairing_safe_compaction_split(body) else {
            return Ok(None);
        };
        let prefix_end = system_end + split;
        let (summary_block, summary_usage) = self.summarize_zone(&body[..split]).await;
        Ok(Some(PrefireNote {
            note: summary_block,
            prefix_end,
            fingerprint: fingerprint_prefix(&messages[..prefix_end]),
            usage: summary_usage,
        }))
    }

    async fn prefire_pass1_response_items(
        &self,
        _instructions: Option<&str>,
        items: &[ResponseInputItem],
    ) -> Result<Option<PrefireNote>> {
        let Some(split) = pairing_safe_response_item_compaction_split(items) else {
            return Ok(None);
        };
        let (summary_block, summary_usage) =
            self.summarize_response_item_zone(&items[..split]).await;
        Ok(Some(PrefireNote {
            note: summary_block,
            prefix_end: split,
            fingerprint: fingerprint_response_item_prefix(&items[..split]),
            usage: summary_usage,
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
            "[Earlier conversation compacted to summary]\n{}",
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
        self.record_durable_boundary(
            deepagent_core::compaction_strategy::PREFIRE_PASS2,
            tokens_before,
            tokens_after,
            &note.note,
            tail.first(),
        );
        Ok(Some(ReactiveCompaction {
            messages: compacted,
            response_items: None,
            tokens_before,
            tokens_after,
            summary: note.note.clone(),
            // Pass-1 paid for this text; carry the charge to the boundary that
            // uses it.
            usage: note.usage,
        }))
    }

    async fn apply_prefire_response_items(
        &self,
        note: &PrefireNote,
        instructions: Option<&str>,
        items: &[ResponseInputItem],
    ) -> Result<Option<ReactiveCompaction>> {
        if note.prefix_end == 0 || note.prefix_end > items.len() {
            return Ok(None);
        }
        if fingerprint_response_item_prefix(&items[..note.prefix_end]) != note.fingerprint {
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

        let count = |items: &[ResponseInputItem]| self.measure_native_items(instructions, items);
        let tokens_before = count(items);

        let tail = &items[note.prefix_end..];
        let mut compacted_items = Vec::with_capacity(1 + tail.len());
        compacted_items.push(ResponseInputItem::Message {
            role: "user".to_string(),
            content: format!("[Earlier conversation compacted to summary]\n{}", note.note),
        });
        compacted_items.extend_from_slice(tail);

        let tokens_after = count(&compacted_items);
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
        self.record_native_boundary(
            deepagent_core::compaction_strategy::PREFIRE_PASS2,
            tokens_before,
            tokens_after,
            &note.note,
            tail,
        )?;
        let messages =
            deepagent_models::messages_from_response_items(instructions, &compacted_items);
        Ok(Some(ReactiveCompaction {
            messages,
            response_items: Some(compacted_items),
            tokens_before,
            tokens_after,
            summary: note.note.clone(),
            // Pass-1 paid for this text; carry the charge to the boundary that
            // uses it.
            usage: note.usage,
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

/// Choose the compaction cut from the token budget, growing the verbatim tail
/// backwards from the newest entry.
///
/// Claude Code `calculateMessagesToKeepIndex`: keep expanding until the tail
/// carries `min_tokens` **and** `min_text_block_entries` text-bearing entries,
/// and stop at `max_tokens` because a bigger tail is the reason the next request
/// overflows again. `counts` and `text_bearing` measure the same slice entry by
/// entry, which is what lets the `Message` view and the response-item view share
/// this single rule -- and the committed pass and its speculative prefire must
/// share it too, or the prefire note is never reusable and the run pays twice.
fn budgeted_tail_split(
    counts: &[usize],
    text_bearing: &[bool],
    window: CompactionKeepWindow,
) -> Option<usize> {
    let mut split = counts.len();
    let mut tokens = 0usize;
    let mut blocks = 0usize;
    while split > 0 {
        let meets_minimums = tokens >= window.min_tokens && blocks >= window.min_text_block_entries;
        if meets_minimums || tokens >= window.max_tokens {
            break;
        }
        let next = split - 1;
        tokens += counts[next];
        blocks += usize::from(text_bearing[next]);
        split = next;
    }
    // A tail that swallows the whole body frees nothing: no compaction to run.
    (split > 0).then_some(split)
}

/// Walk the cut back until the retained tail does not start on a tool result;
/// its requesting assistant turn has to travel with it, or the provider rejects
/// the request over an orphaned output.
fn retain_whole_tool_pair(
    mut split: usize,
    len: usize,
    is_output: impl Fn(usize) -> bool,
) -> usize {
    while split > 0 && split < len && is_output(split) {
        split -= 1;
    }
    split
}

/// Claude Code `hasTextBlocks`: only prose counts toward the retained *thread*.
/// Tool traffic and reasoning do not, so a tail of tool dumps can never satisfy
/// the text minimum on its own.
fn response_item_carries_text(item: &ResponseInputItem) -> bool {
    match item {
        ResponseInputItem::Message { role, content }
        | ResponseInputItem::InputMessage { role, content, .. } => {
            matches!(role.as_str(), "user" | "assistant") && !content.trim().is_empty()
        }
        _ => false,
    }
}

fn pairing_safe_response_item_compaction_split(items: &[ResponseInputItem]) -> Option<usize> {
    let counter = HeuristicTokenizer::new();
    let counts = items
        .iter()
        .map(|item| {
            deepagent_context::TokenCounter::count(
                &counter,
                &render_response_item_for_compaction(item),
            )
        })
        .collect::<Vec<_>>();
    let text = items
        .iter()
        .map(response_item_carries_text)
        .collect::<Vec<_>>();
    let split = budgeted_tail_split(&counts, &text, ContextPolicy::KEEP_WINDOW)?;
    let mut split = split;
    // Parallel calls can have results on either side of the proposed cut.
    // Pull the boundary back to the earliest requesting item for every kept result.
    loop {
        let before = split;
        for (index, item) in items[..split].iter().enumerate() {
            let call_id = match item {
                ResponseInputItem::FunctionCall { call_id, .. }
                | ResponseInputItem::CustomToolCall { call_id, .. } => call_id,
                _ => continue,
            };
            if items[split..].iter().any(|item| match item {
                ResponseInputItem::FunctionCallOutput { call_id: id, .. }
                | ResponseInputItem::CustomToolCallOutput { call_id: id, .. } => id == call_id,
                _ => false,
            }) {
                split = split.min(index);
            }
        }
        if split > 0 && matches!(items[split - 1], ResponseInputItem::Reasoning { .. }) {
            split -= 1;
        }
        if split == before {
            break;
        }
    }
    (split > 0).then_some(split)
}

#[cfg(test)]
fn is_tool_output_item(item: &ResponseInputItem) -> bool {
    matches!(
        item,
        ResponseInputItem::FunctionCallOutput { .. }
            | ResponseInputItem::CustomToolCallOutput { .. }
    )
}

#[cfg(test)]
fn micro_compact_response_item_outputs(
    items: &[ResponseInputItem],
    split: usize,
) -> Option<(Vec<ResponseInputItem>, usize)> {
    const MIN_CLEAR_CHARS: usize = 240;
    const CLEARED_STUB: &str = r#"{"status":"cleared","note":"tool result cleared by micro-compact to free context; re-run the tool if this data is needed again"}"#;

    let mut cleared = 0usize;
    let mut out = items.to_vec();
    for item in out.iter_mut().take(split) {
        match item {
            ResponseInputItem::FunctionCallOutput { output, .. }
            | ResponseInputItem::CustomToolCallOutput { output, .. }
                if output.chars().count() > MIN_CLEAR_CHARS =>
            {
                *output = CLEARED_STUB.to_string();
                cleared += 1;
            }
            _ => {}
        }
    }
    (cleared > 0).then_some((out, cleared))
}

fn render_response_item_for_compaction(item: &ResponseInputItem) -> String {
    match item {
        ResponseInputItem::Message { role, content } => format!("{role}: {content}"),
        ResponseInputItem::InputMessage { role, content, .. } => format!("{role}: {content}"),
        ResponseInputItem::Reasoning { content, .. } => format!("reasoning: {content}"),
        ResponseInputItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => format!("function_call call_id={call_id} name={name} arguments={arguments}"),
        ResponseInputItem::FunctionCallOutput { call_id, output } => {
            format!("function_call_output call_id={call_id} output={output}")
        }
        ResponseInputItem::CustomToolCall {
            call_id,
            name,
            input,
        } => format!("custom_tool_call call_id={call_id} name={name} input={input}"),
        ResponseInputItem::CustomToolCallOutput { call_id, output } => {
            format!("custom_tool_call_output call_id={call_id} output={output}")
        }
        ResponseInputItem::WebSearchCall { id, status, action } => {
            let action = action
                .as_ref()
                .map(serde_json::Value::to_string)
                .unwrap_or_default();
            format!("web_search_call id={id} status={status} action={action}")
        }
    }
}

fn latest_user_content_from_response_items(items: &[ResponseInputItem]) -> Option<String> {
    items.iter().rev().find_map(|item| match item {
        ResponseInputItem::Message { role, content } if role == "user" => Some(content.clone()),
        _ => None,
    })
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
#[cfg(test)]
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

fn compaction_reinjection_block_from_items(compacted_zone: &[ResponseInputItem]) -> Option<String> {
    const MAX_ITEMS: usize = 8;
    let mut files: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut skills: Vec<String> = Vec::new();
    let mut call_names: HashMap<String, String> = HashMap::new();

    for item in compacted_zone {
        match item {
            ResponseInputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                call_names.insert(call_id.clone(), name.clone());
                let parsed =
                    serde_json::from_str::<serde_json::Value>(arguments).unwrap_or_default();
                if REINJECTION_WRITE_TOOLS.contains(&name.as_str()) {
                    if let Some(path) = parsed
                        .get("path")
                        .or_else(|| parsed.get("file_path"))
                        .or_else(|| parsed.get("destination"))
                        .and_then(|value| value.as_str())
                    {
                        let rendered = path.to_string();
                        if !files.contains(&rendered) && files.len() < MAX_ITEMS {
                            files.push(rendered);
                        }
                    }
                }
                if name == deepagent_builtins::SKILL_TOOL_NAME {
                    if let Some(id) = parsed.get("id").and_then(|value| value.as_str()) {
                        let rendered = id.to_string();
                        if !skills.contains(&rendered) && skills.len() < MAX_ITEMS {
                            skills.push(rendered);
                        }
                    }
                }
            }
            ResponseInputItem::CustomToolCall { call_id, name, .. } => {
                call_names.insert(call_id.clone(), name.clone());
            }
            ResponseInputItem::FunctionCallOutput { call_id, output }
            | ResponseInputItem::CustomToolCallOutput { call_id, output }
                if output.contains("\"status\":\"error\"") && failures.len() < MAX_ITEMS =>
            {
                let tool = call_names
                    .get(call_id)
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());
                let preview: String = output.chars().take(160).collect();
                failures.push(format!("{tool}: {preview}"));
            }
            _ => {}
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
    /// Aggregated MCP server instructions, surfaced as `McpCatalog` context.
    /// This is the full-block fallback: when per-session delta mode is disabled
    /// (or no entries are available) the whole block is sent every turn.
    pub(crate) mcp_instructions_block: Option<&'a str>,
    /// Unrendered `(server_name, instructions)` pairs for per-session delta
    /// rendering. When present, only servers not yet announced this session
    /// (reconstructed from `prior_events`) are rendered, mirroring Claude Code's
    /// `mcpInstructionsDelta`. `None` keeps the `mcp_instructions_block` path.
    pub(crate) mcp_instructions_entries: Option<&'a [(String, String)]>,
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
        deferred_tools_announcement(&request.tool_manifest.undiscovered_deferred_sources);
    let skill_catalog_blocks = build_skill_catalog_blocks(
        request.skills,
        request.settings,
        request.skill_catalog_state,
        request.session_id,
        request.prior_events,
    )?;

    let mcp_instructions_block = mcp_instructions_block_for_run(
        request.mcp_instructions_block,
        request.mcp_instructions_entries,
        request.prior_events,
    );

    // Load user language preference from settings (Claude Code pattern).
    let user_language = request.settings.load().ok().flatten().and_then(|s| {
        let lang = s.language.trim();
        if lang.is_empty() {
            None
        } else {
            Some(lang.to_string())
        }
    });

    let system_manifest = build_system_manifest(
        request.root,
        request.sandbox_mode,
        output_style_block,
        plugin_output_style_block,
        tool_catalog_block,
        mcp_instructions_block,
        skill_catalog_blocks,
        request.context_policy.prompt_budget,
        user_language.as_deref(),
    );
    let system_prompt = system_manifest.render();

    // User-input layer attachments (knowledge passive block, remote context)
    // are collected through the shared attachment hub rather than hand-composed.
    let mut registry =
        deepagent_context::AttachmentRegistry::new(USER_INPUT_ATTACHMENT_CHAR_BUDGET);
    if let Some(knowledge) = request.knowledge {
        registry.register(Arc::new(KnowledgeReminderProvider {
            knowledge: knowledge.clone(),
        }));
    }
    if matches!(request.effective_env_mode, Some("remote")) {
        if let (Some(factory), Some(connection_id)) =
            (request.remote_context_factory, request.connection_id)
        {
            registry.register(Arc::new(RemoteContextProvider {
                connection_id: connection_id.to_string(),
                factory: factory.clone(),
            }));
        }
    }
    let attachment_ctx = deepagent_context::AttachmentContext {
        session_id: request.session_id,
        cwd: request.root,
        query: request.prompt_for_model,
        is_subagent: false,
        estimated_tokens: 0,
    };
    let user_attachments = registry
        .collect(
            &attachment_ctx,
            deepagent_context::AttachmentLayer::UserInput,
        )
        .await;
    let final_user_prompt =
        compose_prompt_with_attachments(request.prompt_for_model, &user_attachments);
    let mut context_usage = build_context_pack_snapshot(
        request.context_policy,
        &system_prompt,
        request.history,
        &final_user_prompt,
        request.tools,
        request.context_compacted,
        &HeuristicTokenizer::new(),
    );
    if let Some((prompt, hit, miss)) =
        request
            .prior_events
            .iter()
            .rev()
            .find_map(|event| match &event.payload {
                EventPayload::UsageRecorded {
                    prompt_tokens,
                    prompt_cache_hit_tokens,
                    prompt_cache_miss_tokens,
                    ..
                } => Some((
                    *prompt_tokens,
                    *prompt_cache_hit_tokens,
                    *prompt_cache_miss_tokens,
                )),
                _ => None,
            })
    {
        // The old request has no verifiable native/header anchor after resume.
        // Keep its measured metadata, and remeasure the current Surface in full.
        context_usage.record_provider_usage(prompt, hit, miss);
    }

    Ok(BuiltRunContext {
        system_manifest,
        system_prompt,
        final_user_prompt,
        context_usage,
    })
}

/// Resolve the MCP-instructions block this turn should present.
///
/// With the per-session delta mode enabled (the default) only servers not yet
/// announced to the model are rendered, so a multi-server session no longer
/// re-sends every server's `instructions` on each turn — the context-weight
/// counterpart to Claude Code's `mcpInstructionsDelta` attachment. Either of
/// two conditions falls back to the full-block path:
/// - `DEEPAGENT_MCP_INSTR_DELTA=false` (explicit opt-out), or
/// - no entries are supplied (the older, block-only plumbing).
fn mcp_instructions_block_for_run(
    full: Option<&str>,
    entries: Option<&[(String, String)]>,
    prior_events: &[Event],
) -> Option<String> {
    if !mcp_instructions_delta_enabled() {
        return full.map(str::to_string);
    }
    let Some(entries) = entries else {
        return full.map(str::to_string);
    };
    if entries.is_empty() {
        return None;
    }
    let announced = crate::input_runtime::collect_announced_mcp_servers_from_events(prior_events);
    let added: Vec<(String, String)> = entries
        .iter()
        .filter(|(name, _)| !announced.contains(name))
        .cloned()
        .collect();
    crate::mcp_runtime::render_mcp_instruction_entries(&added)
}

/// Opt-out switch for the per-session MCP-instructions delta. Defaults to on
/// (delta is the context-reduction default); set `DEEPAGENT_MCP_INSTR_DELTA=false`
/// to restore full-block re-sending per turn.
fn mcp_instructions_delta_enabled() -> bool {
    match std::env::var("DEEPAGENT_MCP_INSTR_DELTA") {
        Ok(v) if v.eq_ignore_ascii_case("false") || v == "0" => false,
        Ok(v) if v.eq_ignore_ascii_case("true") || v == "1" => true,
        _ => true,
    }
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

/// Character budget for the user-input attachment layer in one run.
const USER_INPUT_ATTACHMENT_CHAR_BUDGET: usize = 12_000;

/// Knowledge-base passive injection (§3.2) as a user-input attachment.
struct KnowledgeReminderProvider {
    knowledge: Arc<KnowledgeService>,
}

#[async_trait]
impl deepagent_context::AttachmentProvider for KnowledgeReminderProvider {
    fn kind(&self) -> &str {
        "knowledge_passive"
    }

    fn layer(&self) -> deepagent_context::AttachmentLayer {
        deepagent_context::AttachmentLayer::UserInput
    }

    async fn collect(
        &self,
        ctx: &deepagent_context::AttachmentContext<'_>,
    ) -> Vec<deepagent_context::ContextAttachment> {
        let block = self.knowledge.passive_block(ctx.query);
        if block.trim().is_empty() {
            return Vec::new();
        }
        vec![deepagent_context::ContextAttachment::new(
            "knowledge_passive",
            deepagent_context::AttachmentLayer::UserInput,
            120,
            deepagent_context::reminder::wrap(&block),
        )]
    }
}

/// Remote-workspace (SSH) context as a user-input attachment.
struct RemoteContextProvider {
    connection_id: String,
    factory: RemoteContextFactory,
}

#[async_trait]
impl deepagent_context::AttachmentProvider for RemoteContextProvider {
    fn kind(&self) -> &str {
        "remote_context"
    }

    fn layer(&self) -> deepagent_context::AttachmentLayer {
        deepagent_context::AttachmentLayer::UserInput
    }

    async fn collect(
        &self,
        _ctx: &deepagent_context::AttachmentContext<'_>,
    ) -> Vec<deepagent_context::ContextAttachment> {
        match (self.factory)(self.connection_id.clone()).await {
            Ok(Some(block)) if !block.trim().is_empty() => {
                vec![deepagent_context::ContextAttachment::new(
                    "remote_context",
                    deepagent_context::AttachmentLayer::UserInput,
                    130,
                    deepagent_context::reminder::wrap(&block),
                )]
            }
            Ok(_) => Vec::new(),
            Err(err) => {
                tracing::warn!(
                    connection_id = %self.connection_id,
                    error = %err,
                    "failed to collect remote context"
                );
                Vec::new()
            }
        }
    }
}

/// Compose the final user prompt from the collected user-input attachments
/// (already priority-ordered, so the highest-priority block precedes the rest).
fn compose_prompt_with_attachments(
    prompt: &str,
    attachments: &[deepagent_context::ContextAttachment],
) -> String {
    if attachments.is_empty() {
        return prompt.to_string();
    }
    let prefix = attachments
        .iter()
        .map(|attachment| attachment.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{prefix}\n\n{prompt}")
}

/// The same budget over the `Message` view (the chat-completions shape).
fn message_carries_text(message: &Message) -> bool {
    matches!(
        message.role,
        deepagent_core::message::Role::User | deepagent_core::message::Role::Assistant
    ) && !message.content.trim().is_empty()
}

pub(crate) fn pairing_safe_compaction_split(messages: &[Message]) -> Option<usize> {
    let counter = HeuristicTokenizer::new();
    let counts = messages
        .iter()
        .map(|message| {
            deepagent_context::TokenCounter::count(
                &counter,
                &render_message_for_compaction(message),
            )
        })
        .collect::<Vec<_>>();
    let text = messages
        .iter()
        .map(message_carries_text)
        .collect::<Vec<_>>();
    let split = budgeted_tail_split(&counts, &text, ContextPolicy::KEEP_WINDOW)?;
    let split = retain_whole_tool_pair(split, messages.len(), |index| {
        messages[index].role == deepagent_core::message::Role::Tool
    });
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
    use deepagent_context::TokenCounter;

    #[tokio::test]
    async fn native_prune_changes_wire_and_survives_database_reopen() {
        use crate::session_memory::testing::{scripted_client_with_transport, ScriptedTransport};
        use deepagent_runtime::{Agent, ModelAgent};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("surface.sqlite3");
        let db = Arc::new(Database::open(&path).unwrap());
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::create(&db, &clock, None).unwrap();
        let sid = session.id();
        let original = "original-large-tool-result".repeat(4_000);
        let mut items = vec![
            ResponseInputItem::Message {
                role: "user".into(),
                content: "old task".into(),
            },
            ResponseInputItem::FunctionCall {
                call_id: "old-c1".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"a.txt"}"#.into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "old-c1".into(),
                output: original.clone(),
            },
        ];
        for index in 0..10 {
            items.push(ResponseInputItem::Message {
                role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("recent {index}"),
            });
        }
        for item in &items {
            session.append_response_item(item.clone()).unwrap();
        }
        let (client, transport) =
            scripted_client_with_transport(vec![ScriptedTransport::text("done")]);
        let mut agent =
            ModelAgent::new(client.clone(), "deepseek-flash", "rules", "current", vec![])
                .with_response_history(items)
                .with_surface_persistence(Arc::new(SessionSurfaceStore {
                    db: db.clone(),
                    session_id: sid,
                }))
                .with_reactive_compactor(Arc::new(HookedReactiveContextCompactor::new(
                    client,
                    "deepseek-flash".into(),
                    Arc::new(HookRegistry::new()),
                    sid,
                    db.clone(),
                )))
                .with_proactive_compaction(2_000);
        agent.think(0, &[]).await.unwrap();
        let wire = transport.last_prompt();
        assert!(!wire.contains("original-large-tool-result"));
        assert!(wire.contains("tool result content cleared"));
        assert!(wire.contains("read_file") && wire.contains("old-c1"));
        drop(agent);
        drop(session);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let events = deepagent_persistence::event_store::EventStore::new(&reopened)
            .load_session(sid)
            .unwrap();
        assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::ResponseItemAppended { item: ResponseInputItem::FunctionCallOutput { output, .. } } if output == &original)), "raw event remains unchanged");
        assert!(events
            .iter()
            .any(|event| matches!(event.payload, EventPayload::ContextSurfacePatched { .. })));
        let replay = crate::input_runtime::response_items_from_events(&events);
        let replay_wire = deepagent_models::ResponseRequest::from_response_items(
            "deepseek-flash",
            Some("rules".into()),
            replay,
        );
        let replay_wire = serde_json::to_value(replay_wire).unwrap();
        assert!(!replay_wire
            .to_string()
            .contains("original-large-tool-result"));
        assert!(replay_wire
            .to_string()
            .contains("tool result content cleared"));
    }

    #[tokio::test]
    async fn native_summary_keeps_exact_tool_metadata_across_replay() {
        use crate::session_memory::testing::{scripted_client, ScriptedTransport};
        let db = Arc::new(Database::open_in_memory().unwrap());
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::create(&db, &clock, None).unwrap();
        let sid = session.id();
        for index in 0..24 {
            session
                .append_response_item(ResponseInputItem::Message {
                    role: "user".into(),
                    content: format!("turn {index}: {}", "detail ".repeat(520)),
                })
                .unwrap();
        }
        let call = ResponseInputItem::FunctionCall {
            call_id: "retained-c1".into(),
            name: "read_file".into(),
            arguments: " {  \"path\" : \"src/lib.rs\" } ".into(),
        };
        session.append_response_item(call.clone()).unwrap();
        session
            .append_response_item(ResponseInputItem::FunctionCallOutput {
                call_id: "retained-c1".into(),
                output: "file content".into(),
            })
            .unwrap();
        let store = deepagent_persistence::event_store::EventStore::new(&db);
        let before = store.load_session(sid).unwrap();
        let items = crate::input_runtime::response_items_from_events(&before);
        let compactor = HookedReactiveContextCompactor::new(
            scripted_client(vec![ScriptedTransport::text(
                "{\"goal\":\"ship\",\"completed\":[\"checked files\"]}",
            )]),
            "deepseek-flash".into(),
            Arc::new(HookRegistry::new()),
            sid,
            db.clone(),
        );
        let compacted = compactor
            .compact_response_items(
                Some("protected system head"),
                &items,
                CompactionTrigger::ContextOverflow,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(compacted.tokens_after < compacted.tokens_before);
        let compacted_items = compacted.response_items.unwrap();
        assert!(
            compacted_items.contains(&call),
            "arguments must not be reserialized through Message"
        );
        let after = store.load_session(sid).unwrap();
        assert!(after.iter().any(|event| matches!(
            &event.payload,
            EventPayload::ContextCompacted {
                first_kept_sequence: Some(_),
                summary: Some(_),
                ..
            }
        )));
        let replay = crate::input_runtime::response_items_from_events(&after);
        assert_eq!(
            replay, compacted_items,
            "live and durable native summary surfaces agree"
        );
        assert!(
            after.len() > before.len(),
            "original events remain in the log"
        );
    }

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
            attachments: Vec::new(),
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

    /// Phase 2 — the boundary has to be durable on the overflow route too. A
    /// summary only adopted in memory leaves the NEXT run replaying the whole
    /// transcript the previous run already paid to compress.
    #[tokio::test]
    async fn a_reactive_boundary_is_durable_and_the_next_run_replays_from_it() {
        use crate::session_memory::testing::{scripted_client, ScriptedTransport};
        use deepagent_core::event::EventPayload;

        let db = Arc::new(deepagent_persistence::Database::open_in_memory().unwrap());
        let clock = deepagent_core::clock::SystemClock;
        let mut session =
            deepagent_session::Session::create(&db, &clock, Some("reactive-boundary")).unwrap();
        // ~910 tokens per turn (the heuristic counts 4 chars/token), so 24 turns
        // sit near 22k: past the 10k tail floor with a real prefix left to fold.
        // A smaller transcript cannot cut anything and would prove nothing here.
        let filler = "detail ".repeat(520);
        for turn in 0..24 {
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user(format!("turn {turn} {filler}")),
                })
                .unwrap();
        }
        let session_id = session.id();
        let store = deepagent_persistence::event_store::EventStore::new(&db);
        let history = crate::input_runtime::model_history_from_events(
            &store.load_session(session_id).unwrap(),
        );
        assert_eq!(history.len(), 24, "precondition: nothing compacted yet");

        let compactor = HookedReactiveContextCompactor::new(
            scripted_client(vec![ScriptedTransport::text(
                "{\"goal\":\"ship\",\"completed\":[\"wrote code\"]}",
            )]),
            "deepseek-chat".to_string(),
            Arc::new(HookRegistry::new()),
            session_id,
            db.clone(),
        );
        let adopted = compactor
            .compact(&history, CompactionTrigger::ContextOverflow)
            .await
            .unwrap()
            .expect("the overflow pass summarizes the prefix");
        assert!(
            adopted.tokens_after < adopted.tokens_before,
            "{} -> {}",
            adopted.tokens_before,
            adopted.tokens_after
        );

        let reloaded = store.load_session(session_id).unwrap();
        let written = reloaded.iter().find_map(|event| match &event.payload {
            EventPayload::ContextCompacted {
                strategy,
                summary,
                first_kept_sequence,
                ..
            } => Some((strategy.as_str(), summary.clone(), *first_kept_sequence)),
            _ => None,
        });
        let (strategy, summary, cut) = written.expect("the reactive pass wrote a durable boundary");
        assert_eq!(
            strategy,
            deepagent_core::compaction_strategy::REACTIVE_MODEL
        );
        let summary = summary.expect("the boundary carries the summary the model reads");
        assert!(
            summary.contains(&session.transcript_dir().display().to_string()),
            "the durable summary lost the transcript pointer"
        );
        let cut = cut.expect("the boundary names the surviving tail");

        // What the next run rebuilds: the summary standing in for the prefix, and
        // the verbatim tail it kept — never the folded prefix again.
        let view = crate::input_runtime::model_history_from_events(&reloaded);
        assert!(
            view[0]
                .content
                .starts_with(crate::input_runtime::COMPACTED_SUMMARY_HEADER),
            "{:?}",
            view[0].content
        );
        assert!(
            view.len() < history.len(),
            "the boundary did not shorten what the next run replays: {} -> {}",
            history.len(),
            view.len()
        );
        assert_eq!(
            view.last().map(|message| message.content.as_str()),
            history.last().map(|message| message.content.as_str()),
            "the compaction's verbatim tail must survive the replay"
        );
        assert!(
            !view
                .iter()
                .any(|message| message.content == history[0].content),
            "the folded prefix is still in the model view"
        );
        assert!(cut > 0, "the cut point must skip the folded prefix: {cut}");
    }

    /// A boundary is recorded in the session the compactor belongs to, never in
    /// whichever log happens to hold matching text. This is the property that keeps
    /// a sub-agent's compaction from cutting the parent's transcript (the sub-agent
    /// runs on its own session id, and its seeded history is not in that log at all).
    #[tokio::test]
    async fn a_boundary_lands_only_in_the_compactor_own_session() {
        use crate::session_memory::testing::{scripted_client, ScriptedTransport};
        use deepagent_core::event::EventPayload;

        let db = Arc::new(deepagent_persistence::Database::open_in_memory().unwrap());
        let clock = deepagent_core::clock::SystemClock;
        let filler = "detail ".repeat(520);
        let mut donor =
            deepagent_session::Session::create(&db, &clock, Some("matching-log")).unwrap();
        let seeded: Vec<Message> = (0..24)
            .map(|turn| Message::user(format!("turn {turn} {filler}")))
            .collect();
        for message in &seeded {
            donor
                .append(EventPayload::MessageAppended {
                    message: message.clone(),
                })
                .unwrap();
        }
        // The compactor's own session is empty: the text it must match does not
        // exist in its log.
        let own =
            deepagent_session::Session::create(&db, &clock, Some("compactor-session")).unwrap();
        let store = deepagent_persistence::event_store::EventStore::new(&db);

        let compactor = HookedReactiveContextCompactor::new(
            scripted_client(vec![ScriptedTransport::text(
                "{\"goal\":\"ship\",\"completed\":[\"wrote code\"]}",
            )]),
            "deepseek-chat".to_string(),
            Arc::new(HookRegistry::new()),
            own.id(),
            db.clone(),
        );
        let donor_events = store.load_session(donor.id()).unwrap();
        let adopted = compactor
            .compact(
                &crate::input_runtime::model_history_from_events(&donor_events),
                CompactionTrigger::ContextOverflow,
            )
            .await
            .unwrap();
        assert!(adopted.is_some(), "precondition: the pass still summarized");

        for (label, id) in [("compactor", own.id()), ("donor", donor.id())] {
            let events = store.load_session(id).unwrap();
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event.payload, EventPayload::ContextCompacted { .. })),
                "{label} gained a boundary it should not have"
            );
        }
    }

    #[test]
    fn the_cut_grows_to_both_floors_and_stops_at_the_cap() {
        let window = ContextPolicy::KEEP_WINDOW;
        assert_eq!(window.min_tokens, 10_000);
        assert_eq!(window.min_text_block_entries, 5);
        assert_eq!(window.max_tokens, 40_000);

        // One giant recent entry: the cap is reached on the first step, so only
        // it is retained. Keeping the eight-entry "recent window" here is what
        // used to leave the compacted request still over budget.
        let counts = [vec![500; 7], vec![45_000]].concat();
        let text = vec![true; counts.len()];
        assert_eq!(budgeted_tail_split(&counts, &text, window), Some(7));

        // Both minimums gate the cut: ten 1k entries carry enough tokens, but
        // only the last five are prose, so the tail keeps expanding until five
        // text entries and 10k tokens are both inside it.
        let counts = vec![1_000; 12];
        let mut text = vec![false; 7];
        text.extend_from_slice(&[true; 5]);
        let all_text = vec![true; counts.len()];
        assert_eq!(budgeted_tail_split(&counts, &text, window), Some(2));
        // Same tokens, prose all the way back: the token floor alone stops it.
        assert_eq!(budgeted_tail_split(&counts, &all_text, window), Some(2));

        // A body smaller than the floor is not worth compacting: the tail would
        // swallow everything and free nothing.
        let counts = vec![1_000; 4];
        let text = vec![true; 4];
        assert_eq!(budgeted_tail_split(&counts, &text, window), None);
    }

    #[test]
    fn the_cut_never_lands_between_a_tool_request_and_its_result() {
        let items = [
            ResponseInputItem::Message {
                role: "user".into(),
                content: "older".into(),
            },
            ResponseInputItem::FunctionCall {
                call_id: "c1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: "result".into(),
            },
            ResponseInputItem::Message {
                role: "user".into(),
                content: "tail".into(),
            },
        ];
        let is_output = |index: usize| is_tool_output_item(&items[index]);

        // A cut on the result (2) must walk back to the request that made it;
        // a cut already on a request (1) or on prose (3) stays put.
        assert_eq!(retain_whole_tool_pair(2, items.len(), is_output), 1);
        assert_eq!(retain_whole_tool_pair(1, items.len(), is_output), 1);
        assert_eq!(retain_whole_tool_pair(3, items.len(), is_output), 3);

        // Tool traffic never counts as prose, so a tail of dumps alone cannot
        // satisfy the text floor.
        assert!(!response_item_carries_text(&items[1]));
        assert!(!response_item_carries_text(&items[2]));
        assert!(response_item_carries_text(&items[3]));
    }

    #[test]
    fn response_item_micro_compact_keeps_output_item_native() {
        let big = "x".repeat(1_000);
        let items = vec![
            ResponseInputItem::FunctionCall {
                call_id: "c1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: big,
            },
        ];

        let (out, cleared) = micro_compact_response_item_outputs(&items, 2).unwrap();

        assert_eq!(cleared, 1);
        assert!(matches!(
            &out[1],
            ResponseInputItem::FunctionCallOutput { call_id, output }
                if call_id == "c1" && output.contains("cleared by micro-compact")
        ));
    }

    #[test]
    fn response_item_reinjection_reads_raw_arguments_and_failures() {
        let zone = vec![
            ResponseInputItem::FunctionCall {
                call_id: "c1".into(),
                name: "write_file".into(),
                arguments: r#"{"path":"src/lib.rs"}"#.into(),
            },
            ResponseInputItem::FunctionCall {
                call_id: "c2".into(),
                name: "skill".into(),
                arguments: r#"{"id":"docx"}"#.into(),
            },
            ResponseInputItem::FunctionCall {
                call_id: "c3".into(),
                name: "bash".into(),
                arguments: r#"{"command":"cargo test"}"#.into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "c3".into(),
                output: r#"{"status":"error","error":"2 tests failed"}"#.into(),
            },
        ];

        let block = compaction_reinjection_block_from_items(&zone).unwrap();

        assert!(block.contains("src/lib.rs"));
        assert!(block.contains("docx"));
        assert!(block.contains("bash"));
        assert!(block.contains("2 tests failed"));
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
        use deepagent_context::{AttachmentLayer, ContextAttachment};
        let remote = ContextAttachment::new(
            "remote_context",
            AttachmentLayer::UserInput,
            130,
            "<system-reminder>\nremote\n</system-reminder>",
        );
        let knowledge = ContextAttachment::new(
            "knowledge_passive",
            AttachmentLayer::UserInput,
            120,
            "<system-reminder>\nknowledge\n</system-reminder>",
        );
        let composed = compose_prompt_with_attachments("delete temp", &[remote, knowledge]);

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
                KeychainStore::new(crate::secret_store::KEYCHAIN_SERVICE)
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
        // A real session log, because the compactor now writes durable
        // boundaries into one: the seeded turns below are persisted first, so the
        // cut point is resolved against events that actually exist.
        let db = Arc::new(deepagent_persistence::Database::open_in_memory().unwrap());
        let mut live_session = deepagent_session::Session::create(
            &db,
            &deepagent_core::clock::SystemClock,
            Some("real-compaction"),
        )
        .expect("create session");
        let compactor: Arc<dyn ReactiveContextCompactor> =
            Arc::new(HookedReactiveContextCompactor::new(
                client.clone(),
                model.to_string(),
                Arc::new(HookRegistry::new()),
                live_session.id(),
                db.clone(),
            ));

        // Long seeded history: every turn is roughly a kilotoken, so the body
        // sits well past the keep-window floor and there is a prefix left to
        // fold once the tail takes its 10k. Under the old eight-entry rule a toy
        // history compacted; under the token budget only a real overflow-sized
        // one does, which is the shape this path exists for.
        let filler = "record the relevant details with file paths and rationale. ".repeat(60usize);
        let history: Vec<Message> = (0..24)
            .map(|i| {
                if i % 2 == 0 {
                    Message::user(format!(
                        "Earlier step {i}: investigate module {i}. {filler}"
                    ))
                } else {
                    Message::assistant(format!("Step {i}: inspected the module. {filler}"))
                }
            })
            .collect();
        let seeded = history.len();
        for message in &history {
            live_session
                .append(EventPayload::MessageAppended {
                    message: message.clone(),
                })
                .expect("seed the session log");
        }

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
        let compacted = events.iter().find_map(|event| match event {
            RuntimeEvent::ContextCompacted {
                tokens_before,
                tokens_after,
                strategy,
                ..
            } if strategy == "proactive_threshold" || strategy == "prefire_pass2" => {
                Some((*tokens_before, *tokens_after))
            }
            _ => None,
        });
        let (tokens_before, tokens_after) = compacted.unwrap_or_else(|| {
            panic!(
                "expected a proactive ContextCompacted event among {} runtime events",
                events.len()
            )
        });
        // The budget promise, measured on the provider's own numbers: the run
        // kept at least the floor of recent thread, and it did free context.
        assert!(
            tokens_after >= deepagent_context::ContextPolicy::KEEP_WINDOW.min_tokens as u64,
            "the retained tail fell under the floor: {tokens_before} -> {tokens_after}"
        );
        assert!(
            tokens_after < tokens_before,
            "compaction freed nothing: {tokens_before} -> {tokens_after}"
        );
        assert!(matches!(
            decision,
            AgentDecision::Continue
                | AgentDecision::Complete(_)
                | AgentDecision::CompleteMessage(_)
                | AgentDecision::CompleteItems { .. }
                | AgentDecision::CallTool(_)
                | AgentDecision::CallTools(_)
        ));
        // The part that only a real run can show: the summary the provider wrote
        // is in the session log as a boundary, and the next run rebuilds a shorter
        // view from it instead of replaying all 24 seeded turns.
        let reloaded = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(live_session.id())
            .expect("reload the session log");
        let durable = reloaded.iter().find_map(|event| match &event.payload {
            EventPayload::ContextCompacted {
                summary,
                first_kept_sequence,
                ..
            } => Some((summary.clone(), *first_kept_sequence)),
            _ => None,
        });
        let (summary, cut) = durable.expect("the real compaction wrote a durable boundary");
        let summary = summary.expect("with the summary the next run reads");
        assert!(
            summary.contains(&live_session.transcript_dir().display().to_string()),
            "the real boundary summary lost the transcript pointer"
        );
        assert!(cut.is_some(), "the real boundary names no surviving tail");
        let view = crate::input_runtime::model_history_from_events(&reloaded);
        assert!(
            view.len() < seeded,
            "the next run still replays the whole transcript: {} vs {seeded}",
            view.len()
        );
        eprintln!(
            "[real-model] durable boundary replays {seeded} -> {} entries",
            view.len()
        );
        eprintln!("[real-model] proactive compaction fired and the run continued OK");
        eprintln!("[real-model] tokens {tokens_before} -> {tokens_after}");
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
                KeychainStore::new(crate::secret_store::KEYCHAIN_SERVICE)
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
        let system = "You are a helpful assistant. Answer directly.";
        let user = "List the numbers from 1 to 40, one per line, as \"N. <english word>\" \
             (e.g. \"1. one\"). Output only the list.";

        // Tiny max_tokens forces truncation → finish_reason = Length.
        let truncated = client
            .stream_response(
                ResponseRequest::with_instructions_and_user_input(model.to_string(), system, user)
                    .with_max_output_tokens(48),
            )
            .await
            .expect("first (truncated) call");
        let truncated_text = truncated.output_text_projection();
        eprintln!(
            "[real-model] first finish_reason={:?}, content_len={}",
            truncated.finish_reason,
            truncated_text.len()
        );
        assert_eq!(
            truncated.finish_reason,
            Some(FinishReason::Length),
            "tiny max_tokens must truncate the answer"
        );
        assert!(!truncated_text.trim().is_empty());

        // Continuation: partial output + the exact recovery prompt the runtime
        // injects, at a larger budget — the model must resume, not restart.
        let mut continuation_input = vec![
            deepagent_core::response_item::ResponseInputItem::Message {
                role: "user".to_string(),
                content: user.to_string(),
            },
            deepagent_core::response_item::ResponseInputItem::Message {
                role: "assistant".to_string(),
                content: truncated_text,
            },
        ];
        for item in truncated.output_items.iter() {
            if let deepagent_core::response_item::ResponseOutputItem::Reasoning { .. } = item {
                continuation_input.push(item.clone());
            }
        }
        continuation_input.push(deepagent_core::response_item::ResponseInputItem::Message {
            role: "user".to_string(),
            content: "Output token limit hit. Resume directly — no apology, no recap of \
                      what you were doing. Pick up mid-thought if that is where the cut \
                      happened. Break remaining work into smaller pieces."
                .to_string(),
        });
        let resumed = client
            .stream_response(
                ResponseRequest::from_response_items(
                    model.to_string(),
                    Some(system.to_string()),
                    continuation_input,
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
                KeychainStore::new(crate::secret_store::KEYCHAIN_SERVICE)
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
                tier: None,
                importance: None,
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
                KeychainStore::new(crate::secret_store::KEYCHAIN_SERVICE)
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

    fn mcp_event(sid: deepagent_core::id::SessionId, seq: u64, payload: EventPayload) -> Event {
        Event {
            id: deepagent_core::id::EventId::new(),
            session_id: sid,
            sequence: seq,
            timestamp: deepagent_core::clock::Timestamp::from_millis(seq as i64),
            payload,
        }
    }

    /// First turn of a session: no server announced yet, so the whole block is
    /// rendered for the first (and only first) time.
    #[test]
    fn mcp_instructions_renders_full_block_before_any_announcement() {
        let entries: Vec<(String, String)> = vec![
            ("alpha".into(), "do X".into()),
            ("beta".into(), "do Y".into()),
        ];
        let block = mcp_instructions_block_for_run(None, Some(&entries), &[]).unwrap();
        assert!(block.contains("## alpha"));
        assert!(block.contains("## beta"));
        assert!(block.contains("do X"));
        assert!(block.contains("do Y"));
    }

    /// Second turn: `alpha` was announced last turn, so the rendered block only
    /// carries `beta`. Rendered `added` must equal what `mcp_instructions_delta`
    /// reports — the two paths can never disagree.
    #[test]
    fn mcp_instructions_renders_only_unanounced_servers_after_first_turn() {
        let sid = deepagent_core::id::SessionId::nil();
        let entries: Vec<(String, String)> = vec![
            ("alpha".into(), "do X".into()),
            ("beta".into(), "do Y".into()),
        ];
        let prior = vec![mcp_event(
            sid,
            0,
            EventPayload::McpInstructionsDelta {
                added: vec!["alpha".into()],
                removed: vec![],
            },
        )];
        let block = mcp_instructions_block_for_run(None, Some(&entries), &prior).unwrap();
        assert!(!block.contains("## alpha"), "alpha already announced");
        assert!(!block.contains("do X"));
        assert!(block.contains("## beta"));
        assert!(block.contains("do Y"));

        let announced = crate::input_runtime::collect_announced_mcp_servers_from_events(&prior);
        let (added, removed) = crate::input_runtime::mcp_instructions_delta(&entries, &announced);
        assert_eq!(added, vec!["beta".to_string()]);
        assert!(removed.is_empty(), "no server should be retracted here");
    }

    /// A server that disconnected (dropped its instructions) is not rendered by
    /// the delta path — there is nothing new to say — but the retraction is
    /// reported so the persisted event can rebuild the announced set on resume.
    #[test]
    fn mcp_instructions_retracts_disconnected_server() {
        let sid = deepagent_core::id::SessionId::nil();
        // Previously announced both; `beta` is gone from the current connection.
        let prior = vec![mcp_event(
            sid,
            0,
            EventPayload::McpInstructionsDelta {
                added: vec!["alpha".into(), "beta".into()],
                removed: vec![],
            },
        )];
        let entries: Vec<(String, String)> = vec![("alpha".into(), "do X".into())];
        let block = mcp_instructions_block_for_run(None, Some(&entries), &prior);
        assert!(
            block.is_none(),
            "nothing new to render after both were announced"
        );

        let announced = crate::input_runtime::collect_announced_mcp_servers_from_events(&prior);
        let (added, removed) = crate::input_runtime::mcp_instructions_delta(&entries, &announced);
        assert!(added.is_empty());
        assert_eq!(removed, vec!["beta".to_string()]);
    }

    /// Reconnecting a previously-announced server does not re-announce it (the
    /// announced set is cumulative), and the full-block entry path is preserved
    /// as a fallback for callers without entries.
    #[test]
    fn mcp_instructions_resume_reports_no_duplicate_announcement() {
        let sid = deepagent_core::id::SessionId::nil();
        let prior = vec![mcp_event(
            sid,
            0,
            EventPayload::McpInstructionsDelta {
                added: vec!["alpha".into()],
                removed: vec![],
            },
        )];
        // Same entries as turn one; nothing should be freshly announced.
        let entries: Vec<(String, String)> = vec![("alpha".into(), "do X".into())];
        let block = mcp_instructions_block_for_run(None, Some(&entries), &prior);
        assert!(block.is_none());

        // Full-block fallback: entries = None keeps the old behavior verbatim.
        let full = mcp_instructions_block_for_run(Some("full block"), None, &prior);
        assert_eq!(full.as_deref(), Some("full block"));
    }

    /// Token-baseline measurement for the MCP delta (S1-3 / §5): once a few
    /// servers have been announced, the per-turn block carries only the
    /// not-yet-announced remainder, so per-turn MCP-instruction pressure drops
    /// monotonically as the session progresses and converges to nothing.
    #[test]
    fn mcp_instructions_delta_slashes_per_turn_token_pressure() {
        let counter = HeuristicTokenizer::new();
        let make_server = |n: usize| -> (String, String) {
            (
                format!("server-{n}"),
                format!("Use the tools from server-{n} to inspect and modify state. ").repeat(12),
            )
        };
        let entries: Vec<(String, String)> = (0..5).map(make_server).collect();

        let full = crate::mcp_runtime::render_mcp_instruction_entries(&entries)
            .expect("full block renders");
        let full_tokens = counter.count(&full);
        assert!(
            full_tokens > 300,
            "full block over 5 verbose servers is sizable (got {full_tokens})"
        );

        // Turn 2: three already announced → only two remain in the prompt.
        let sid = deepagent_core::id::SessionId::nil();
        let announced = vec![mcp_event(
            sid,
            0,
            EventPayload::McpInstructionsDelta {
                added: vec!["server-0".into(), "server-1".into(), "server-2".into()],
                removed: vec![],
            },
        )];
        let delta_block = mcp_instructions_block_for_run(None, Some(&entries), &announced)
            .expect("two servers remain");
        let delta_tokens = counter.count(&delta_block);
        assert!(
            delta_tokens <= full_tokens * 2 / 3,
            "fewer servers announced must shrink the block (delta {delta_tokens} vs full {full_tokens})"
        );

        // Turn 3: the rest announced → zero new instructions (idempotent
        // convergence, same inputs as before must yield nothing new).
        let rest = [mcp_event(
            sid,
            1,
            EventPayload::McpInstructionsDelta {
                added: vec!["server-3".into(), "server-4".into()],
                removed: vec![],
            },
        )];
        let combined = [announced[0].clone(), rest[0].clone()];
        let converged = mcp_instructions_block_for_run(None, Some(&entries), &combined);
        assert!(
            converged.is_none(),
            "fully-announced session adds no MCP block"
        );
    }
}
