//! Context replacement invalidates tool observations only after adoption.
use super::*;
use deepagent_models::{MockTransport, ModelConfig};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct SurfaceStore {
    reject: AtomicBool,
}

impl ContextSurfacePersistence for SurfaceStore {
    fn replace_tool_results(&self, _: &str, _: &[(String, String)]) -> Result<()> {
        self.record_snip(&[])
    }

    fn record_snip(&self, _: &[String]) -> Result<()> {
        if self.reject.load(Ordering::Acquire) {
            Err(CoreError::other("synthetic Surface persistence rejection"))
        } else {
            Ok(())
        }
    }
}

fn agent() -> (ModelAgent, Arc<AtomicUsize>) {
    let client = Arc::new(ModelClient::new(
        Arc::new(MockTransport::new(vec![])),
        ModelConfig::deepseek("synthetic"),
    ));
    let mut agent = ModelAgent::new(client, "deepseek-flash", "rules", "current", vec![]);
    let resets = Arc::new(AtomicUsize::new(0));
    let counter = resets.clone();
    agent.set_tool_context_reset(Arc::new(move || {
        counter.fetch_add(1, Ordering::AcqRel);
    }));
    (agent, resets)
}

fn stale_result(agent: ModelAgent, custom: bool) -> ModelAgent {
    let mut items = vec![ResponseInputItem::Message {
        role: "user".into(),
        content: "old task".into(),
    }];
    if custom {
        items.push(ResponseInputItem::CustomToolCall {
            call_id: "old-read".into(),
            name: "read_file".into(),
            input: "fixture.txt".into(),
        });
        items.push(ResponseInputItem::CustomToolCallOutput {
            call_id: "old-read".into(),
            output: "observed bytes ".repeat(200),
        });
    } else {
        items.push(ResponseInputItem::FunctionCall {
            call_id: "old-read".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        });
        items.push(ResponseInputItem::FunctionCallOutput {
            call_id: "old-read".into(),
            output: "observed bytes ".repeat(200),
        });
    }
    items.extend((0..10).map(|index| ResponseInputItem::Message {
        role: "assistant".into(),
        content: format!("protected {index}"),
    }));
    agent.with_response_history(items)
}

#[test]
fn full_adoptions_and_explicit_history_replacements_reset_once() {
    for native in [false, true] {
        for strategy in ["proactive_threshold", "reactive_model", "prefire_pass2"] {
            let (mut agent, resets) = agent();
            agent.adopt_compaction(
                ReactiveCompaction {
                    messages: vec![Message::system("rules"), Message::user("summary")],
                    response_items: native.then(|| {
                        vec![ResponseInputItem::Message {
                            role: "user".into(),
                            content: "summary".into(),
                        }]
                    }),
                    tokens_before: 100,
                    tokens_after: 10,
                    summary: "summary".into(),
                    usage: Default::default(),
                },
                strategy,
            );
            assert_eq!(resets.load(Ordering::Acquire), 1);
            assert!(agent
                .conversation()
                .iter()
                .any(|message| message.content == "summary"));
            agent = agent.with_history(vec![]).with_response_history(vec![]);
            assert_eq!(resets.load(Ordering::Acquire), 1);
            agent = agent.with_history(vec![Message::user("restored")]);
            assert_eq!(resets.load(Ordering::Acquire), 2);
            let _ = agent.with_response_history(vec![ResponseInputItem::Message {
                role: "user".into(),
                content: "other branch".into(),
            }]);
            assert_eq!(resets.load(Ordering::Acquire), 3);
        }
    }
}

#[test]
fn tool_prune_keeps_authority_on_persistence_failure_and_on_empty_replacements() {
    for custom in [false, true] {
        let (agent, resets) = agent();
        let store = Arc::new(SurfaceStore::default());
        let mut agent = stale_result(agent, custom).with_surface_persistence(store.clone());
        resets.store(0, Ordering::Release);
        let original = agent.response_history.items.clone();
        store.reject.store(true, Ordering::Release);
        assert!(agent
            .prune_stale_tool_results(60, "tool_result_prune")
            .is_err());
        assert_eq!(agent.response_history.items, original);
        assert_eq!(resets.load(Ordering::Acquire), 0);
        store.reject.store(false, Ordering::Release);
        assert_eq!(
            agent
                .prune_stale_tool_results(60, "tool_result_prune")
                .unwrap(),
            1
        );
        assert_eq!(resets.load(Ordering::Acquire), 1);
        assert_eq!(agent.clear_stale_tool_results().unwrap(), 0);
        assert_eq!(resets.load(Ordering::Acquire), 1);
    }
}

#[test]
fn idle_microcompact_revokes_only_after_a_real_replacement() {
    let (agent, resets) = agent();
    let mut agent = stale_result(agent, false);
    resets.store(0, Ordering::Release);
    let last = std::time::Instant::now();
    agent.last_assistant_at = Some(last);
    let threshold = std::time::Duration::from_secs(10);
    agent.maybe_time_based_microcompact_at(last + threshold / 2, threshold);
    assert_eq!(resets.load(Ordering::Acquire), 0);
    agent.maybe_time_based_microcompact_at(last + threshold * 2, threshold);
    assert_eq!(resets.load(Ordering::Acquire), 1);
    agent.maybe_time_based_microcompact_at(last + threshold * 3, threshold);
    assert_eq!(resets.load(Ordering::Acquire), 1);
}

#[test]
fn snip_retains_authority_until_nonempty_plan_is_persisted() {
    let (agent, resets) = agent();
    let history = (0..12)
        .map(|index| {
            if index % 2 == 0 {
                Message::user(format!("turn {index}"))
            } else {
                Message::assistant(format!("answer {index}"))
            }
        })
        .collect();
    let store = Arc::new(SurfaceStore::default());
    let mut agent = agent
        .with_history(history)
        .with_snip_tool("snip_history")
        .with_surface_persistence(store.clone());
    resets.store(0, Ordering::Release);
    let original = agent.response_history.items.clone();
    for output in [
        serde_json::json!({}),
        serde_json::json!({"ids":[]}),
        serde_json::json!({"ids":["missing", "u99"]}),
    ] {
        agent.apply_snip_from_output(&output);
        assert_eq!(resets.load(Ordering::Acquire), 0);
    }
    store.reject.store(true, Ordering::Release);
    agent.apply_snip_from_output(&serde_json::json!({"ids":["u1"]}));
    assert_eq!(agent.response_history.items, original);
    assert_eq!(resets.load(Ordering::Acquire), 0);
    store.reject.store(false, Ordering::Release);
    agent.apply_snip_from_output(&serde_json::json!({"ids":["u1"]}));
    assert!(agent.response_history.items.len() < original.len());
    assert_eq!(resets.load(Ordering::Acquire), 1);
    agent.apply_snip_from_output(&serde_json::json!({"ids":["u1"]}));
    assert_eq!(resets.load(Ordering::Acquire), 1);
}
