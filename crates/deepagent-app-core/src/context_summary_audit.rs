//! Audit the structured-summary adoption boundary, not model summarization quality.

use super::*;
use crate::session_memory::testing::{scripted_client_with_transport, ScriptedTransport};
use deepagent_runtime::{Agent, ModelAgent};

const FACT: &str = "AUDIT_SUMMARY_FACT_268";

async fn check_summary_adoption(label: &str, model_summary: &str) {
    check_summary_adoption_for_fact(label, model_summary, FACT).await;
}

async fn check_summary_adoption_for_fact(label: &str, model_summary: &str, fact: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("summary.sqlite3");
    let db = Arc::new(Database::open(&path).unwrap());
    let mut session =
        deepagent_session::Session::create(&db, &deepagent_core::clock::SystemClock, None).unwrap();
    let sid = session.id();
    let items: Vec<_> = (0..24)
        .map(|index| ResponseInputItem::Message {
            role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
            content: if index == 1 {
                // A fact that the existing heuristic fallback also retains.
                format!("Created {fact}\n{}", "historical details ".repeat(600))
            } else {
                format!("unique turn {index}: {}", "historical details ".repeat(600))
            },
        })
        .collect();
    for item in &items {
        session.append_response_item(item.clone()).unwrap();
    }
    let current = ResponseInputItem::Message {
        role: "user".into(),
        content: "current".into(),
    };
    session.append_response_item(current.clone()).unwrap();
    let mut expected = items.clone();
    expected.push(current);
    let split = pairing_safe_response_item_compaction_split(&expected).unwrap();
    assert!(split > 1, "the fact must actually be in the summary zone");
    assert!(!serde_json::to_string(&expected[split..])
        .unwrap()
        .contains(fact));

    let (summary_client, summary_transport) =
        scripted_client_with_transport(vec![ScriptedTransport::text(model_summary)]);
    let (main_client, main_transport) =
        scripted_client_with_transport(vec![ScriptedTransport::text("done")]);
    let compactor = Arc::new(HookedReactiveContextCompactor::new(
        summary_client,
        "deepseek-flash".into(),
        Arc::new(HookRegistry::new()),
        sid,
        db.clone(),
    ));
    let mut agent = ModelAgent::new(
        main_client,
        "deepseek-flash",
        "protected rules",
        "current",
        vec![],
    )
    .with_response_history(items)
    .with_snip_tool(deepagent_builtins::SNIP_HISTORY_TOOL_NAME)
    .with_reactive_compactor(compactor.clone())
    .with_proactive_compaction(2_000);
    tokio::time::timeout(std::time::Duration::from_secs(5), agent.think(0, &[]))
        .await
        .expect("audit model/compaction did not finish")
        .unwrap();
    assert!(summary_transport.last_prompt().contains(fact));
    let request: serde_json::Value = serde_json::from_str(&main_transport.last_prompt()).unwrap();
    let sent: Vec<ResponseInputItem> = serde_json::from_value(request["input"].clone()).unwrap();
    assert_eq!(request["instructions"], "protected rules");
    let side_usage = agent.side_usage().unwrap();
    assert_eq!(side_usage.billed_prompt_tokens, 120);
    assert_eq!(side_usage.completion_tokens, 18);
    drop(agent);
    drop(compactor);
    drop(session);
    drop(db);

    // Close every database owner before testing the cold recovery projection.
    let reopened = Database::open(&path).unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&reopened)
        .load_session(sid)
        .unwrap();
    let boundaries: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ContextCompacted {
                tokens_before,
                tokens_after,
                summary,
                ..
            } => Some((*tokens_before, *tokens_after, summary.clone().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(boundaries.len(), 1);
    let (before, after, summary) = &boundaries[0];
    assert!(after < before);
    let replay = crate::input_runtime::response_items_from_events(&events);
    assert!(
        replay == sent,
        "live and cold-recovered surfaces must agree"
    );
    let raw_fact = events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::ResponseItemAppended {
                item: ResponseInputItem::Message { content, .. }
            } if content.starts_with(&format!("Created {fact}"))
        )
    });
    let wire_fact = request.to_string().contains(fact);
    let replay_fact = serde_json::to_string(&replay).unwrap().contains(fact);
    let summary_fact = summary.contains(fact);
    eprintln!(
        "{label}: split={split}, tokens={before}->{after}, boundaries={}, raw_fact={raw_fact}, summary_fact={summary_fact}, wire_fact={wire_fact}, replay_fact={replay_fact}, side_usage=120+18",
        boundaries.len(),
    );
    assert!(
        raw_fact,
        "the append-only source evidence must remain intact"
    );
    assert!(
        summary.contains("完整记录（未压缩的原始事件）"),
        "the recovery pointer must remain"
    );
    assert!(
        wire_fact && replay_fact && summary_fact,
        "summary adoption must not silently remove the supplied checkpoint fact"
    );
}

#[tokio::test]
async fn valid_structured_summary_preserves_fact() {
    let output = serde_json::json!({
        "goal": "continue audit",
        "completed": [format!("Created {FACT}")],
        "pending": [], "decisions": [], "failures": [],
    });
    check_summary_adoption("valid", &output.to_string()).await;
}

#[tokio::test]
async fn non_json_summary_uses_existing_heuristic_fallback() {
    check_summary_adoption("non-json-fallback", "not a JSON summary").await;
}

#[tokio::test]
#[ignore = "audit: empty JSON must not silently shadow the original history"]
async fn audit_empty_object_does_not_shadow_history() {
    check_summary_adoption("empty-object", "{}").await;
}

#[tokio::test]
#[ignore = "audit: unrelated JSON must not silently shadow the original history"]
async fn audit_unrelated_object_does_not_shadow_history() {
    check_summary_adoption("unrelated-object", r#"{"unrelated":"not a checkpoint"}"#).await;
}

#[tokio::test]
#[ignore = "audit: empty progress sections must not silently shadow the original history"]
async fn audit_empty_sections_do_not_shadow_history() {
    check_summary_adoption(
        "empty-sections",
        r#"{"goal":"","completed":[],"pending":[],"decisions":[],"failures":[]}"#,
    )
    .await;
}

const LITERAL_FACT: &str = "<analysis>AUDIT_SUMMARY_LITERAL_269</analysis>";

fn literal_checkpoint(fact: &str) -> String {
    serde_json::json!({
        "goal": "continue audit",
        "completed": [format!("Created {fact}")],
        "pending": ["next audit"],
        "decisions": ["preserve the literal template text"],
        "failures": [],
    })
    .to_string()
}

#[tokio::test]
async fn literal_summary_escaped_analysis_tag_survives() {
    let literal = literal_checkpoint(LITERAL_FACT);
    let escaped = literal.replace('<', "\\u003c");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&literal).unwrap(),
        serde_json::from_str::<serde_json::Value>(&escaped).unwrap(),
        "the two summary bodies must describe exactly the same JSON value"
    );
    check_summary_adoption_for_fact("unicode-escaped-analysis", &escaped, LITERAL_FACT).await;
}

#[tokio::test]
async fn literal_summary_other_tag_survives() {
    let fact = "<summary>AUDIT_SUMMARY_LITERAL_269</summary>";
    check_summary_adoption_for_fact("literal-summary-tag", &literal_checkpoint(fact), fact).await;
}

#[tokio::test]
async fn literal_summary_leading_scratchpad_still_allows_checkpoint() {
    let body = format!(
        "<analysis>AUDIT_DRAFT_ONLY_269</analysis>\n{}",
        literal_checkpoint(FACT)
    );
    check_summary_adoption("leading-scratchpad", &body).await;
}

#[tokio::test]
#[ignore = "audit: scratch cleanup must not delete literal text inside valid JSON strings"]
async fn audit_literal_summary_analysis_tag_does_not_erase_fact() {
    check_summary_adoption_for_fact(
        "literal-analysis-tag",
        &literal_checkpoint(LITERAL_FACT),
        LITERAL_FACT,
    )
    .await;
}
