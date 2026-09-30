//! Tag-based conversation-segment snipping, shared by the live model loop and
//! by history rebuild.
//!
//! The model can invoke the history-snip tool with `[id:uN]` tags to drop
//! earlier, clearly-finished segments and free context. The same removal must
//! be re-applied when a session's history is rebuilt from its event log after a
//! restart (Claude Code `removedUuids` parity), so the segment math lives here
//! rather than inline in the loop.

use deepagent_core::message::{Message, Role};

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
        let Some(start) = messages
            .iter()
            .position(|m| m.role == Role::User && m.content.contains(&tag))
        else {
            continue;
        };
        if start == 0 || start >= protected_start {
            continue;
        }
        let mut end = messages[start + 1..protected_start]
            .iter()
            .position(|m| m.role == Role::User && m.content.contains("[id:u"))
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
    use super::*;

    fn user(text: &str) -> Message {
        Message::user(text)
    }
    fn assistant(text: &str) -> Message {
        Message::assistant(text)
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
}
