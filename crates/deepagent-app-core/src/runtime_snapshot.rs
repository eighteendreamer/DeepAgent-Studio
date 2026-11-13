//! Split the existing manifest; persist updates through the existing Session.
//! Snapshot replacement wording/versioning is a local contract, not a claim
//! that a provider automatically replaces historical system messages.

use deepagent_core::response_item::ResponseItem;
use deepagent_core::runtime_snapshot::{latest_snapshot, RuntimeSnapshot};
use deepagent_core::{CoreError, Result};
use sha2::{Digest, Sha256};

pub(crate) struct SnapshotPlan {
    pub instructions: String,
    pub update: Option<ResponseItem>,
    pub restarted: bool,
}

impl SnapshotPlan {
    pub fn persist<C: deepagent_core::clock::Clock>(
        &self,
        session: &mut deepagent_session::Session<'_, C>,
    ) -> Result<()> {
        if let Some(item) = &self.update {
            session.append_host_context(item.clone())?;
            tracing::info!(session_id = %session.id(), cache_series_restarted = self.restarted,
                "committed versioned runtime context snapshot");
        }
        Ok(())
    }

    pub fn new(manifest: &str, history: &[ResponseItem]) -> Result<Self> {
        if history.iter().any(|item| {
            deepagent_core::runtime_snapshot::RuntimeSnapshot::has_envelope_marker(item)
                && deepagent_core::runtime_snapshot::RuntimeSnapshot::from_item(item).is_none()
        }) {
            return Err(CoreError::invalid(
                "session contains an unsupported or malformed runtime context snapshot",
            ));
        }
        let Some((stable, dynamic)) =
            manifest.split_once(crate::system_context::SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
        else {
            return Ok(Self {
                instructions: manifest.into(),
                update: None,
                restarted: false,
            });
        };
        let hash = format!("{:x}", Sha256::digest(stable.as_bytes()));
        let previous = latest_snapshot(history).map(|(_, snapshot)| snapshot);
        let restarted = previous
            .as_ref()
            .is_some_and(|s| s.stable_rules_sha256 != hash);
        let update = match previous {
            Some(s) if !restarted && s.context == dynamic => None,
            previous => {
                let (series, revision) = match previous {
                    Some(s) if restarted => (s.series.checked_add(1), Some(1)),
                    Some(s) => (Some(s.series), s.revision.checked_add(1)),
                    None => (Some(1), Some(1)),
                };
                Some(
                    RuntimeSnapshot {
                        version: 1,
                        series: series.ok_or_else(|| {
                            CoreError::invalid("runtime snapshot series overflow")
                        })?,
                        revision: revision.ok_or_else(|| {
                            CoreError::invalid("runtime snapshot revision overflow")
                        })?,
                        stable_rules_sha256: hash,
                        context: dynamic.into(),
                    }
                    .to_item(),
                )
            }
        };
        Ok(Self {
            instructions: stable.into(),
            update,
            restarted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(stable: &str, dynamic: &str) -> String {
        format!(
            "{stable}{}{dynamic}",
            crate::system_context::SYSTEM_PROMPT_DYNAMIC_BOUNDARY
        )
    }

    #[test]
    fn h168_complete_snapshot_dedup_update_and_stable_rule_restart() {
        let first = SnapshotPlan::new(&manifest("rules", "env v1"), &[]).unwrap();
        assert_eq!(first.instructions, "rules");
        assert!(!first.restarted);
        let mut history = vec![first.update.unwrap()];
        assert!(SnapshotPlan::new(&manifest("rules", "env v1"), &history)
            .unwrap()
            .update
            .is_none());
        for dynamic in ["env v2", "", "env v1"] {
            let plan = SnapshotPlan::new(&manifest("rules", dynamic), &history).unwrap();
            assert_eq!(plan.instructions, "rules");
            let item = plan.update.unwrap();
            let s = RuntimeSnapshot::from_item(&item).unwrap();
            assert_eq!(s.series, 1);
            assert_eq!(s.revision, history.len() as u64 + 1);
            assert_eq!(s.context, dynamic);
            history.push(item);
        }
        let plan = SnapshotPlan::new(&manifest("new stable rules", "env v1"), &history).unwrap();
        assert!(plan.restarted);
        let s = RuntimeSnapshot::from_item(&plan.update.unwrap()).unwrap();
        assert_eq!((s.series, s.revision), (2, 1));
        assert_eq!(plan.instructions, "new stable rules");
    }

    #[test]
    fn h168_malformed_or_unknown_host_snapshot_fails_closed() {
        let malformed = ResponseItem::Message {
            role: "system".into(),
            content: "[DeepAgent runtime context snapshot]\n{\"version\":99}".into(),
        };
        assert!(SnapshotPlan::new(&manifest("rules", "current"), &[malformed]).is_err());
    }

    #[test]
    fn h168_bootstrap_legacy_and_rewind_use_only_visible_history() {
        let legacy = vec![ResponseItem::Message {
            role: "user".into(),
            content: "old turn".into(),
        }];
        let first = SnapshotPlan::new(&manifest("rules", "full context"), &legacy).unwrap();
        let mut history = legacy.clone();
        history.push(first.update.unwrap());
        let second = SnapshotPlan::new(&manifest("rules", "new context"), &history).unwrap();
        history.push(second.update.unwrap());
        history.pop(); // Rewind/fork retains only the chosen event prefix.
        assert!(
            SnapshotPlan::new(&manifest("rules", "full context"), &history)
                .unwrap()
                .update
                .is_none()
        );
        assert_eq!(legacy.len(), 1); // No historical event rewrite.
    }

    #[test]
    fn h168_snapshot_persistence_failure_escapes_before_adoption() {
        use deepagent_persistence::Database;
        let db = Database::open_in_memory().unwrap();
        let clock = deepagent_core::clock::SystemClock;
        let mut session = deepagent_session::Session::create(&db, &clock, None).unwrap();
        let plan = SnapshotPlan::new(&manifest("rules", "env"), &[]).unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "DELETE FROM sessions WHERE id=?1",
                [session.id().to_string()],
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        let result = plan.persist(&mut session);
        assert!(
            result.is_err(),
            "failed snapshot must not be adopted as committed context"
        );
    }

    #[tokio::test]
    async fn h168_production_compactor_pins_full_snapshot_for_all_native_paths() {
        use crate::context_runtime::HookedReactiveContextCompactor;
        use crate::session_memory::testing::{scripted_client, ScriptedTransport};
        use deepagent_core::{clock::SystemClock, event::EventPayload, message::Message};
        use deepagent_persistence::{event_store::EventStore, Database};
        use deepagent_runtime::{CompactionTrigger, ReactiveContextCompactor};
        use deepagent_session::Session;
        use std::sync::Arc;
        for mode in 0..3 {
            let db = Arc::new(Database::open_in_memory().unwrap());
            let clock = SystemClock;
            let mut session = Session::create(&db, &clock, None).unwrap();
            let snapshot =
                SnapshotPlan::new(&manifest("rules", "full current state"), &[]).unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("first"),
                })
                .unwrap();
            snapshot.persist(&mut session).unwrap();
            for n in 0..30 {
                session
                    .append_response_item(ResponseItem::Message {
                        role: if n % 2 == 0 { "user" } else { "assistant" }.into(),
                        content: format!("turn {n}: {}", "synthetic context ".repeat(80)),
                    })
                    .unwrap();
            }
            let source = crate::input_runtime::response_items_from_events(
                &EventStore::new(&db).load_session(session.id()).unwrap(),
            );
            let compactor = HookedReactiveContextCompactor::new(
                scripted_client(vec![ScriptedTransport::text(
                    r#"{"goal":"synthetic summary","completed":["old work"]}"#,
                )]),
                "deepseek-flash".into(),
                Arc::new(deepagent_hooks::HookRegistry::new()),
                session.id(),
                db.clone(),
            );
            let compacted = if mode == 2 {
                let note = compactor
                    .prefire_pass1_response_items(Some("rules"), &source)
                    .await
                    .unwrap()
                    .unwrap();
                compactor
                    .apply_prefire_response_items(&note, Some("rules"), &source)
                    .await
                    .unwrap()
                    .unwrap()
            } else {
                compactor
                    .compact_response_items(
                        Some("rules"),
                        &source,
                        if mode == 0 {
                            CompactionTrigger::ContextOverflow
                        } else {
                            CompactionTrigger::AutoCompactThreshold
                        },
                    )
                    .await
                    .unwrap()
                    .unwrap()
            };
            let items = compacted.response_items.unwrap();
            assert!(compacted.tokens_after < compacted.tokens_before);
            assert_eq!(
                items
                    .iter()
                    .filter(|item| RuntimeSnapshot::from_item(item).is_some())
                    .count(),
                1
            );
            assert_eq!(
                latest_snapshot(&items).unwrap().1.context,
                "full current state"
            );
            let events = EventStore::new(&db).load_session(session.id()).unwrap();
            assert_eq!(
                crate::input_runtime::response_items_from_events(&events),
                items,
                "native path {mode} must replay exactly"
            );
            let expected =
                deepagent_context::measure_response_surface(Some("rules"), &items, &[]).total();
            assert_eq!(compacted.tokens_after as usize, expected);
        }
    }

    #[test]
    fn h168_real_session_compaction_carry_order_cold_reopen_fork_and_rewind() {
        use crate::input_runtime::{response_items_from_events, sequence_of_response_item_tail};
        use deepagent_core::{clock::SystemClock, event::EventPayload, message::Message};
        use deepagent_persistence::{event_store::EventStore, Database};
        use deepagent_session::Session;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        let clock = SystemClock;
        let (id, first_sequence, expected, original) = {
            let db = Database::open(&path).unwrap();
            let mut session = Session::create(&db, &clock, None).unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("first"),
                })
                .unwrap();
            let first = SnapshotPlan::new(&manifest("rules", "env1"), &[])
                .unwrap()
                .update
                .unwrap();
            let first_sequence = session.append_response_item(first).unwrap().sequence;
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("second"),
                })
                .unwrap();
            let source = response_items_from_events(
                &EventStore::new(&db).load_session(session.id()).unwrap(),
            );
            let latest = SnapshotPlan::new(&manifest("rules", "env2"), &source)
                .unwrap()
                .update
                .unwrap();
            session.append_response_item(latest.clone()).unwrap();
            session
                .append(EventPayload::MessageAppended {
                    message: Message::user("kept tail"),
                })
                .unwrap();
            let events = EventStore::new(&db).load_session(session.id()).unwrap();
            let items = response_items_from_events(&events);
            let tail = vec![items.last().unwrap().clone()];
            let cut = sequence_of_response_item_tail(&events, &tail).unwrap();
            session
                .append(EventPayload::ContextCompacted {
                    tokens_before: 100,
                    tokens_after: 20,
                    strategy: "proactive".into(),
                    summary: Some("old turns".into()),
                    first_kept_sequence: Some(cut),
                })
                .unwrap();
            let mut expected = vec![ResponseItem::Message {
                role: "user".into(),
                content: "[Earlier conversation compacted to summary]\nold turns".into(),
            }];
            expected.extend(tail);
            expected.push(latest);
            let after = ResponseItem::Message {
                role: "assistant".into(),
                content: "answer after compaction".into(),
            };
            session.append_response_item(after.clone()).unwrap();
            expected.push(after);
            let events = EventStore::new(&db).load_session(session.id()).unwrap();
            assert_eq!(response_items_from_events(&events), expected);
            assert!(
                sequence_of_response_item_tail(&events, &expected[1..]).is_some(),
                "carried snapshot does not corrupt next compaction's tail identity"
            );
            (session.id(), first_sequence, expected, events)
        };
        let db = Database::open(&path).unwrap();
        let events = EventStore::new(&db).load_session(id).unwrap();
        assert_eq!(response_items_from_events(&events), expected);
        assert_eq!(events, original, "cold replay never rewrites raw history");
        let fork = Session::fork(&db, &clock, id, first_sequence).unwrap();
        let fork_items =
            response_items_from_events(&EventStore::new(&db).load_session(fork.id()).unwrap());
        assert_eq!(latest_snapshot(&fork_items).unwrap().1.context, "env1");
        let mut session = Session::recover(&db, &clock, id).unwrap();
        session.rewind(first_sequence).unwrap();
        let rewound = response_items_from_events(&EventStore::new(&db).load_session(id).unwrap());
        assert_eq!(latest_snapshot(&rewound).unwrap().1.context, "env1");
        assert!(
            !SnapshotPlan::new(&manifest("rules", "env1"), &rewound)
                .unwrap()
                .restarted
        );
    }
}
