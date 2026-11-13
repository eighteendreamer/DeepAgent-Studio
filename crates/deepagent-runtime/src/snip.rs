//! Tag-based conversation-segment snipping, shared by the live model loop and
//! by history rebuild.
//!
//! The model can invoke the history-snip tool with `[id:uN]` tags to drop
//! earlier, clearly-finished segments and free context. The same removal must
//! be re-applied when a session's history is rebuilt from its event log after a
//! restart (Claude Code `removedUuids` parity), so the segment math lives here
//! rather than inline in the loop.

use deepagent_core::message::{Message, Role};
use deepagent_core::response_item::ResponseInputItem;

/// Number of most-recent messages never snipped: the live exchange (and its
/// tool pairing) must survive.
pub const SNIP_PROTECTED_RECENT_MESSAGES: usize = 8;

/// Planned removal over a conversation for a set of snipped tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnipPlan {
    /// One flag per input message: `true` means removed.
    pub remove: Vec<bool>,
    /// Tags whose segment was actually found and scheduled for removal.
    pub applied_tags: Vec<String>,
}

/// Planned removal over provider-native response items.
///
/// The native item view is the request source of truth. Keeping this planner
/// beside the legacy `Message` planner makes live snips and session replay use
/// the same segment rules without rebuilding tool calls through a lossy chat
/// projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseItemSnipPlan {
    /// One flag per response item: `true` means removed.
    pub remove: Vec<bool>,
    /// Tags whose segment was actually found and scheduled for removal.
    pub applied_tags: Vec<String>,
}

impl ResponseItemSnipPlan {
    /// Whether anything would actually be removed.
    pub fn removes_anything(&self) -> bool {
        self.remove.iter().any(|flag| *flag)
    }

    /// Apply the plan, returning the kept response items.
    pub fn apply(self, items: Vec<ResponseInputItem>) -> Vec<ResponseInputItem> {
        items
            .into_iter()
            .enumerate()
            .filter_map(|(index, item)| (!self.remove[index]).then_some(item))
            .collect()
    }
}

fn user_content(item: &ResponseInputItem) -> Option<&str> {
    match item {
        ResponseInputItem::Message { role, content }
        | ResponseInputItem::InputMessage { role, content, .. }
            if role == "user"
                && !content.starts_with(deepagent_core::response_item::HOOK_CONTEXT_PREFIX) =>
        {
            Some(content)
        }
        _ => None,
    }
}

/// Assign stable tags before any model-view cuts are applied. Existing tags
/// are preserved; callers restoring events must tag the complete item log.
pub fn tag_response_user_items(items: &mut [ResponseInputItem]) {
    let mut next_id = items
        .iter()
        .filter_map(user_content)
        .filter_map(|content| {
            content
                .rsplit_once("[id:u")
                .and_then(|(_, rest)| rest.split(']').next())
                .and_then(|id| id.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0);
    for item in items {
        let content = match item {
            ResponseInputItem::Message { role, content }
            | ResponseInputItem::InputMessage { role, content, .. }
                if role == "user" =>
            {
                content
            }
            _ => continue,
        };
        if content.contains("[id:u")
            || content.starts_with('[')
            || content.starts_with("<system-reminder>")
        {
            continue;
        }
        next_id += 1;
        content.push_str(&format!("\n[id:u{next_id}]"));
    }
}

/// Compute which native response items a set of tags would remove.
///
/// The retained tail is never allowed to begin with a tool output. If a
/// protected boundary lands inside a call/result pair, the boundary moves
/// backwards so the complete pair remains in the retained tail.
pub fn plan_response_item_snip(
    items: &[ResponseInputItem],
    tags: &[String],
    protected_recent: usize,
) -> ResponseItemSnipPlan {
    let protected_start = items.len().saturating_sub(protected_recent);
    let mut remove = vec![false; items.len()];
    let mut applied_tags = Vec::new();

    for id in tags {
        let tag = format!("[id:{id}]");
        let Some(start) = items
            .iter()
            .position(|item| user_content(item).is_some_and(|content| content.contains(&tag)))
        else {
            continue;
        };
        if start >= protected_start {
            continue;
        }

        let Some(offset) = items[start + 1..]
            .iter()
            .position(|item| user_content(item).is_some_and(|content| content.contains("[id:u")))
        else {
            continue;
        };
        let end = start + 1 + offset;
        // Only complete tagged segments are removed. A partial segment would
        // grow on later replay, deleting a tail the live run retained.
        if end > protected_start {
            continue;
        }
        let crosses_pair = items.iter().enumerate().any(|(index, item)| {
            let Some(call_id) = item.client_tool_call_id() else {
                return false;
            };
            items.iter().enumerate().any(|(result_index, result)| {
                result.client_tool_output_call_id() == Some(call_id)
                    && (start..end).contains(&index) != (start..end).contains(&result_index)
            })
        });
        if crosses_pair {
            continue;
        }
        applied_tags.push(id.clone());
        for flag in remove.iter_mut().take(end).skip(start) {
            *flag = true;
        }
    }

    // Host system snapshots are not user-turn payload. Keep their original
    // positions, including historical revisions: otherwise replaying a snip
    // after a later update could delete the snapshot the live snip retained.
    // Compaction can discard superseded snapshots and carry only the latest.
    for (index, item) in items.iter().enumerate() {
        if deepagent_core::runtime_snapshot::RuntimeSnapshot::has_envelope_marker(item) {
            remove[index] = false;
        }
    }
    ResponseItemSnipPlan {
        remove,
        applied_tags,
    }
}

/// Convenience: drop tagged segments from provider-native response items.
pub fn snip_response_items(
    items: &[ResponseInputItem],
    tags: &[String],
    protected_recent: usize,
) -> Vec<ResponseInputItem> {
    plan_response_item_snip(items, tags, protected_recent).apply(items.to_vec())
}

impl SnipPlan {
    /// Whether anything would actually be removed.
    pub fn removes_anything(&self) -> bool {
        self.remove.iter().any(|flag| *flag)
    }

    /// Apply the plan, returning the kept messages.
    pub fn apply(self, messages: Vec<Message>) -> Vec<Message> {
        messages
            .into_iter()
            .enumerate()
            .filter_map(|(index, message)| (!self.remove[index]).then_some(message))
            .collect()
    }
}

/// Compute which messages a set of `tags` would remove.
///
/// Each tag matches a user turn whose content carries `[id:<tag>]`; its segment
/// runs to the next tagged user turn or the protected tail. The first message
/// (index 0) and the last `protected_recent` messages are never removed.
/// Pairing safety: a retained tail must not begin with a tool result whose
/// requesting assistant sits inside the removal zone, so the boundary is pulled
/// back over such results.
pub fn plan_snip(messages: &[Message], tags: &[String], protected_recent: usize) -> SnipPlan {
    let protected_start = messages.len().saturating_sub(protected_recent);
    let mut remove = vec![false; messages.len()];
    let mut applied_tags = Vec::new();
    for id in tags {
        let tag = format!("[id:{id}]");
        let Some(start) = messages.iter().position(|m| {
            m.role == Role::User
                && !m
                    .content
                    .starts_with(deepagent_core::response_item::HOOK_CONTEXT_PREFIX)
                && m.content.contains(&tag)
        }) else {
            continue;
        };
        if start == 0 || start >= protected_start {
            continue;
        }
        let mut end = messages[start + 1..protected_start]
            .iter()
            .position(|m| {
                m.role == Role::User
                    && !m
                        .content
                        .starts_with(deepagent_core::response_item::HOOK_CONTEXT_PREFIX)
                    && m.content.contains("[id:u")
            })
            .map(|offset| start + 1 + offset)
            .unwrap_or(protected_start);
        while end > start && messages.get(end).is_some_and(|m| m.role == Role::Tool) {
            end -= 1;
        }
        if end <= start {
            continue;
        }
        applied_tags.push(id.clone());
        for flag in remove.iter_mut().take(end).skip(start) {
            *flag = true;
        }
    }
    SnipPlan {
        remove,
        applied_tags,
    }
}

/// Convenience: drop the tagged segments from `messages`.
pub fn snip_segments(
    messages: &[Message],
    tags: &[String],
    protected_recent: usize,
) -> Vec<Message> {
    plan_snip(messages, tags, protected_recent).apply(messages.to_vec())
}

#[cfg(test)]
mod tests {
    #[test]
    fn h168_snip_keeps_host_snapshots_so_later_updates_do_not_change_replay() {
        use deepagent_core::response_item::ResponseItem;
        let snapshot = |revision| {
            deepagent_core::runtime_snapshot::RuntimeSnapshot {
                version: 1,
                series: 1,
                revision,
                stable_rules_sha256: "a".repeat(64),
                context: format!("state {revision}"),
            }
            .to_item()
        };
        let user = |id| ResponseItem::Message {
            role: "user".into(),
            content: format!("turn [id:u{id}]"),
        };
        let items = vec![user(1), snapshot(1), snapshot(2), user(2), user(3)];
        let kept = super::snip_response_items(&items, &["u1".into()], 1);
        assert_eq!(kept, vec![snapshot(1), snapshot(2), user(2), user(3)]);
        let mut later = items;
        later.push(snapshot(3));
        let mut expected = kept;
        expected.push(snapshot(3));
        assert_eq!(
            super::snip_response_items(&later, &["u1".into()], 1),
            expected
        );
    }
    use super::*;

    fn user(text: &str) -> Message {
        Message::user(text)
    }
    fn assistant(text: &str) -> Message {
        Message::assistant(text)
    }

    fn native_user(text: &str) -> ResponseInputItem {
        ResponseInputItem::Message {
            role: "user".into(),
            content: text.into(),
        }
    }

    #[test]
    fn native_snip_never_removes_a_partial_segment() {
        let items = vec![native_user("[id:u1] old"), native_user("[id:u2] latest")];
        let plan = plan_response_item_snip(&items, &["u2".into()], 0);
        assert!(
            !plan.removes_anything(),
            "latest tagged segment has no durable end boundary"
        );
    }

    #[test]
    fn native_snip_rejects_a_cut_through_a_parallel_pair() {
        let items = vec![
            native_user("[id:u1] old"),
            ResponseInputItem::FunctionCall {
                call_id: "c1".into(),
                name: "read_file".into(),
                arguments: "{}".into(),
            },
            native_user("[id:u2] next"),
            ResponseInputItem::FunctionCallOutput {
                call_id: "c1".into(),
                output: "result".into(),
            },
        ];
        assert!(!plan_response_item_snip(&items, &["u1".into()], 0).removes_anything());
    }

    #[test]
    fn h152_discovery_snip_retains_crossing_pair_and_removes_a_complete_pair() {
        let call: ResponseInputItem = serde_json::from_value(serde_json::json!({
            "type":"tool_search_call", "call_id":"discovery", "execution":"client",
            "status":"completed", "arguments":{"query":"calendar"}
        }))
        .unwrap();
        let output: ResponseInputItem = serde_json::from_value(serde_json::json!({
            "type":"tool_search_output", "call_id":"discovery", "execution":"client",
            "status":"completed", "tools":[]
        }))
        .unwrap();
        let crossing = vec![
            native_user("[id:u1] old"),
            call.clone(),
            native_user("[id:u2] next"),
            output.clone(),
        ];
        assert!(!plan_response_item_snip(&crossing, &["u1".into()], 0).removes_anything());
        let complete = vec![
            native_user("[id:u1] old"),
            call,
            output,
            native_user("[id:u2] next"),
        ];
        let plan = plan_response_item_snip(&complete, &["u1".into()], 1);
        assert_eq!(plan.applied_tags, ["u1"]);
        assert_eq!(plan.apply(complete), [native_user("[id:u2] next")]);
    }

    #[test]
    fn native_snip_preserves_unrelated_tool_pair_metadata() {
        let items = vec![
            native_user("first"),
            native_user("[id:u1] old task"),
            ResponseInputItem::FunctionCall {
                call_id: "old-call".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"old"}"#.into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "old-call".into(),
                output: "old result".into(),
            },
            native_user("[id:u2] live task"),
            ResponseInputItem::FunctionCall {
                call_id: "live-call".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"live"}"#.into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "live-call".into(),
                output: "live result".into(),
            },
        ];

        let plan = plan_response_item_snip(&items, &["u1".into()], 2);
        assert_eq!(plan.applied_tags, vec!["u1"]);
        let kept = plan.apply(items);
        assert!(kept.iter().any(|item| matches!(
            item,
            ResponseInputItem::FunctionCall { call_id, name, arguments }
                if call_id == "live-call" && name == "read_file" && arguments.contains("live")
        )));
        assert!(kept.iter().any(|item| matches!(
            item,
            ResponseInputItem::FunctionCallOutput { call_id, .. } if call_id == "live-call"
        )));
        assert!(!kept.iter().any(|item| matches!(
            item,
            ResponseInputItem::FunctionCall { call_id, .. } if call_id == "old-call"
        )));
    }

    #[test]
    fn removes_segment_between_tagged_turns() {
        let messages = vec![
            user("first"),        // 0: protected (index 0)
            user("[id:u1] do a"), // 1
            assistant("a done"),  // 2
            user("[id:u2] do b"), // 3
            assistant("b done"),  // 4
            assistant("tail"),    // 5
        ];
        let plan = plan_snip(&messages, &["u1".to_string()], 0);
        assert_eq!(plan.applied_tags, vec!["u1".to_string()]);
        let kept = plan.apply(messages);
        // Segment [1..3) removed: u1 turn and its assistant reply.
        assert_eq!(kept.len(), 4);
        assert!(kept.iter().all(|m| !m.content.contains("[id:u1]")));
    }

    #[test]
    fn protected_tail_and_first_message_survive() {
        let messages = vec![
            user("[id:u1] goal"),  // 0: never removed
            assistant("r1"),       // 1
            user("[id:u2] goal2"), // 2
            assistant("r2"),       // 3
        ];
        // Protect the last 3 → only index 0 could be in range, and it's index 0.
        let plan = plan_snip(&messages, &["u1".to_string(), "u2".to_string()], 3);
        assert!(!plan.removes_anything());
    }

    #[test]
    fn unknown_tag_yields_no_removal() {
        let messages = vec![user("[id:u1] a"), assistant("b")];
        let plan = plan_snip(&messages, &["nope".to_string()], 0);
        assert!(plan.applied_tags.is_empty());
        assert!(!plan.removes_anything());
    }
    #[test]
    fn h140_auxiliary_hook_context_stays_in_its_source_snip_segment() {
        let context = format!(
            "{}{{\"text\":\"untrusted [id:u999]\"}}",
            deepagent_core::response_item::HOOK_CONTEXT_PREFIX
        );
        let mut items = vec![
            native_user("source turn"),
            native_user(&context),
            native_user("next turn"),
            ResponseInputItem::Message {
                role: "assistant".into(),
                content: "answer".into(),
            },
        ];
        tag_response_user_items(&mut items);
        assert!(user_content(&items[0]).unwrap().contains("[id:u1]"));
        assert!(user_content(&items[2]).unwrap().contains("[id:u2]"));
        assert!(matches!(&items[1],ResponseInputItem::Message {content,..} if *content==context));
        assert!(plan_response_item_snip(&items, &["u999".into()], 0)
            .applied_tags
            .is_empty());
        let plan = plan_response_item_snip(&items, &["u1".into()], 0);
        assert_eq!(plan.remove, vec![true, true, false, false]);
        let messages = deepagent_models::messages_from_response_items(None, &items);
        assert!(plan_snip(&messages, &["u999".into()], 0)
            .applied_tags
            .is_empty());
    }
}
