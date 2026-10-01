//! Unified context-attachment hub (aligned with Claude Code's `attachments.ts`).
//!
//! The runtime and the app layer previously built model-visible injections
//! (todo reminders, snip/doom-loop nudges, relevant memories, catalogs, remote
//! context, …) each in its own place, with its own budget/dedup/throttle rules.
//! This module gives them one shape: providers implement [`AttachmentProvider`]
//! (async, like CC's `getAttachments` providers) and an [`AttachmentRegistry`]
//! collects their [`ContextAttachment`]s under a single dedup + character-budget
//! policy.
//!
//! The types live in `deepagent-context` so both `deepagent-runtime` and
//! `deepagent-app-core` can share them (the runtime must not depend on the app
//! layer).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// Which slice of a turn an attachment targets. Providers declare one layer;
/// the caller collects only the layers relevant to the current point in the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentLayer {
    /// Only when processing fresh user input (e.g. knowledge/@-mentions).
    UserInput,
    /// Any thread — main run or sub-agent (e.g. todo/skill nudges).
    AllThread,
    /// Main run only (e.g. background memory prefetch).
    MainThread,
}

/// A structured, model-visible attachment produced by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextAttachment {
    /// Stable provider kind (e.g. `"todo_reminder"`), for logging and dedup.
    pub kind: String,
    /// Which layer this attachment belongs to.
    pub layer: AttachmentLayer,
    /// Higher survives character-budget pressure longer.
    pub priority: u8,
    /// Ready-to-inject body. A provider that needs the `<system-reminder>`
    /// envelope should wrap via [`crate::reminder::wrap`] before returning.
    pub content: String,
    /// When set, the registry surfaces this attachment at most once per session
    /// (for announce-once kinds). `None` means the provider self-throttles and
    /// the attachment may recur.
    pub dedup_key: Option<String>,
}

impl ContextAttachment {
    /// Build an attachment that may recur (provider self-throttles).
    pub fn new(
        kind: impl Into<String>,
        layer: AttachmentLayer,
        priority: u8,
        content: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            layer,
            priority,
            content: content.into(),
            dedup_key: None,
        }
    }

    /// Mark this attachment as announce-once under `key`.
    pub fn with_dedup_key(mut self, key: impl Into<String>) -> Self {
        self.dedup_key = Some(key.into());
        self
    }
}

/// Read-only view a provider uses to decide what to attach.
pub struct AttachmentContext<'a> {
    /// Session id (for per-session dedup and provider state).
    pub session_id: &'a str,
    /// Effective working directory.
    pub cwd: &'a Path,
    /// The current user query / prompt for this turn (empty for non-user
    /// collection points).
    pub query: &'a str,
    /// Whether the current run is a sub-agent (some attachments are main-only).
    pub is_subagent: bool,
    /// Estimated tokens used this turn (for pace-based providers).
    pub estimated_tokens: u64,
}

/// A source of context attachments.
#[async_trait::async_trait]
pub trait AttachmentProvider: Send + Sync {
    /// Stable provider id, used for logging and dedup bookkeeping.
    fn kind(&self) -> &str;

    /// The layer this provider contributes to.
    fn layer(&self) -> AttachmentLayer {
        AttachmentLayer::AllThread
    }

    /// Produce attachments for `ctx`. Implementations must not panic; return an
    /// empty vec when there is nothing to attach. Synchronous providers simply
    /// return without awaiting.
    async fn collect(&self, ctx: &AttachmentContext<'_>) -> Vec<ContextAttachment>;
}

/// Collects attachments from registered providers under a dedup + character
/// budget policy.
#[derive(Default)]
pub struct AttachmentRegistry {
    providers: Vec<Arc<dyn AttachmentProvider>>,
    /// Total content-character budget across one collection.
    char_budget: usize,
    /// Dedup keys already surfaced this session.
    seen: HashSet<String>,
}

impl AttachmentRegistry {
    /// New registry with a total character budget for a single collection pass.
    pub fn new(char_budget: usize) -> Self {
        Self {
            providers: Vec::new(),
            char_budget,
            seen: HashSet::new(),
        }
    }

    /// Register a provider (builder style).
    pub fn with_provider(mut self, provider: Arc<dyn AttachmentProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Register a provider in place.
    pub fn register(&mut self, provider: Arc<dyn AttachmentProvider>) -> &mut Self {
        self.providers.push(provider);
        self
    }

    /// Clear per-session dedup state (e.g. on a new session).
    pub fn reset_seen(&mut self) {
        self.seen.clear();
    }

    /// Collect attachments for `layer` from every provider whose layer matches.
    ///
    /// Providers are queried sequentially in registration order (deterministic),
    /// already-seen `dedup_key`s are dropped, and survivors are fitted to the
    /// character budget by dropping the lowest-priority attachments first, then
    /// returned with higher priority first (ties keep provider order).
    pub async fn collect(
        &mut self,
        ctx: &AttachmentContext<'_>,
        layer: AttachmentLayer,
    ) -> Vec<ContextAttachment> {
        let mut collected: Vec<(usize, ContextAttachment)> = Vec::new();
        for (index, provider) in self.providers.iter().enumerate() {
            if !layer_matches(provider.layer(), layer) {
                continue;
            }
            for attachment in provider.collect(ctx).await {
                if let Some(key) = attachment.dedup_key.as_ref() {
                    if self.seen.contains(key) {
                        continue;
                    }
                }
                collected.push((index, attachment));
            }
        }

        // Fit to budget: drop lowest priority first (stable by provider order).
        let mut total: usize = collected
            .iter()
            .map(|(_, a)| a.content.chars().count())
            .sum();
        if total > self.char_budget {
            let mut by_priority: Vec<usize> = (0..collected.len()).collect();
            by_priority.sort_by_key(|&i| (collected[i].1.priority, collected[i].0));
            for i in by_priority {
                if total <= self.char_budget {
                    break;
                }
                total = total.saturating_sub(collected[i].1.content.chars().count());
                collected[i].1.content.clear();
            }
            collected.retain(|(_, a)| !a.content.is_empty());
        }

        // Record dedup keys of survivors, then order higher-priority first.
        for (_, attachment) in &collected {
            if let Some(key) = attachment.dedup_key.as_ref() {
                self.seen.insert(key.clone());
            }
        }
        collected.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then(a.0.cmp(&b.0)));
        collected.into_iter().map(|(_, a)| a).collect()
    }
}

fn layer_matches(provider: AttachmentLayer, requested: AttachmentLayer) -> bool {
    match requested {
        AttachmentLayer::AllThread => matches!(
            provider,
            AttachmentLayer::AllThread | AttachmentLayer::MainThread
        ),
        _ => provider == requested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedProvider {
        kind: String,
        layer: AttachmentLayer,
        attachments: Vec<ContextAttachment>,
    }

    #[async_trait::async_trait]
    impl AttachmentProvider for FixedProvider {
        fn kind(&self) -> &str {
            &self.kind
        }
        fn layer(&self) -> AttachmentLayer {
            self.layer
        }
        async fn collect(&self, _ctx: &AttachmentContext<'_>) -> Vec<ContextAttachment> {
            self.attachments.clone()
        }
    }

    fn ctx() -> AttachmentContext<'static> {
        AttachmentContext {
            session_id: "s1",
            cwd: Path::new("/work"),
            query: "do the thing",
            is_subagent: false,
            estimated_tokens: 0,
        }
    }

    #[tokio::test]
    async fn collects_from_matching_layer_only() {
        let mut registry = AttachmentRegistry::new(10_000).with_provider(Arc::new(FixedProvider {
            kind: "main".into(),
            layer: AttachmentLayer::MainThread,
            attachments: vec![ContextAttachment::new(
                "main",
                AttachmentLayer::MainThread,
                100,
                "m",
            )],
        }));
        registry.register(Arc::new(FixedProvider {
            kind: "input".into(),
            layer: AttachmentLayer::UserInput,
            attachments: vec![ContextAttachment::new(
                "input",
                AttachmentLayer::UserInput,
                100,
                "u",
            )],
        }));

        let all = registry.collect(&ctx(), AttachmentLayer::AllThread).await;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].kind, "main");

        let input = registry.collect(&ctx(), AttachmentLayer::UserInput).await;
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].kind, "input");
    }

    #[tokio::test]
    async fn dedup_key_surfaces_at_most_once() {
        let mut registry = AttachmentRegistry::new(10_000).with_provider(Arc::new(FixedProvider {
            kind: "skill_lint".into(),
            layer: AttachmentLayer::AllThread,
            attachments: vec![ContextAttachment::new(
                "skill_lint",
                AttachmentLayer::AllThread,
                100,
                "once",
            )
            .with_dedup_key("lint")],
        }));
        assert_eq!(
            registry
                .collect(&ctx(), AttachmentLayer::AllThread)
                .await
                .len(),
            1
        );
        assert!(registry
            .collect(&ctx(), AttachmentLayer::AllThread)
            .await
            .is_empty());
        registry.reset_seen();
        assert_eq!(
            registry
                .collect(&ctx(), AttachmentLayer::AllThread)
                .await
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn budget_drops_lowest_priority_first() {
        let mut registry = AttachmentRegistry::new(5).with_provider(Arc::new(FixedProvider {
            kind: "mixed".into(),
            layer: AttachmentLayer::AllThread,
            attachments: vec![
                ContextAttachment::new("low", AttachmentLayer::AllThread, 10, "aaaaa"),
                ContextAttachment::new("high", AttachmentLayer::AllThread, 200, "bbbbb"),
            ],
        }));
        let kept = registry.collect(&ctx(), AttachmentLayer::AllThread).await;
        // Only one fits in a 5-char budget; the high-priority one survives.
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].kind, "high");
    }

    #[tokio::test]
    async fn results_are_priority_ordered() {
        let mut registry = AttachmentRegistry::new(10_000).with_provider(Arc::new(FixedProvider {
            kind: "order".into(),
            layer: AttachmentLayer::AllThread,
            attachments: vec![
                ContextAttachment::new("a", AttachmentLayer::AllThread, 10, "a"),
                ContextAttachment::new("b", AttachmentLayer::AllThread, 200, "b"),
            ],
        }));
        let kept = registry.collect(&ctx(), AttachmentLayer::AllThread).await;
        assert_eq!(kept[0].kind, "b");
        assert_eq!(kept[1].kind, "a");
    }
}
