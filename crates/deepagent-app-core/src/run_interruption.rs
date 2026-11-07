//! Read-only run/task attribution over the existing authoritative session log.
use deepagent_core::event::{Event, EventPayload, Sequence};
use deepagent_core::id::TaskId;
use deepagent_core::response_item::ResponseItem;

/// Admission persists user input (and its native projection) before TaskCreated.
/// Include that unclaimed prompt, but never a previous task's input or a later
/// prompt/task. Caller verifies the task has exactly one run owner. Unknown or
/// duplicate task creation is not evidence for borrowing the session's tail.
pub(super) fn classify_for_task(
    events: &[Event],
    items: &[(Sequence, ResponseItem)],
    task: TaskId,
) -> Option<&'static str> {
    let mut creations = events.iter().enumerate().filter_map(|(index, event)| {
        matches!(&event.payload, EventPayload::TaskCreated { task_id, .. } if *task_id == task)
            .then_some(index)
    });
    let created = creations.next()?;
    if creations.next().is_some() {
        return None;
    }
    let previous_task = events[..created]
        .iter()
        .rposition(|event| matches!(event.payload, EventPayload::TaskCreated { .. }))
        .map(|index| events[index].sequence);
    let created_sequence = events[created].sequence;
    let start = items
        .iter()
        .rev()
        .find(|(sequence, item)| {
            *sequence < created_sequence
                && previous_task.map_or(true, |previous| *sequence > previous)
                && is_user_input(item)
        })
        .map_or(created_sequence, |(sequence, _)| *sequence);
    let next_task = events[created + 1..]
        .iter()
        .find(|event| matches!(event.payload, EventPayload::TaskCreated { .. }))
        .map(|event| event.sequence);
    let next_input = items
        .iter()
        .find(|(sequence, item)| *sequence > created_sequence && is_user_input(item))
        .map(|(sequence, _)| *sequence);
    let end = next_input.into_iter().chain(next_task).min();
    classify_items(
        items
            .iter()
            .filter(|(sequence, _)| *sequence >= start && end.map_or(true, |end| *sequence < end))
            .map(|(_, item)| item),
    )
}

fn is_user_input(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { role, .. } | ResponseItem::InputMessage { role, .. } => {
            role == "user"
        }
        _ => false,
    }
}

/// Same decisive-item rules, using the existing source/native normalization.
/// This is diagnostic, never a decision to resume or execute work.
/// None means a settled assistant reply or no attributable decisive event.
#[cfg(test)]
pub(super) fn classify_tail(events: &[Event]) -> Option<&'static str> {
    let items = crate::input_runtime::indexed_response_items(events);
    classify_items(items.iter().map(|(_, item)| item))
}

fn classify_items<'a>(
    items: impl DoubleEndedIterator<Item = &'a ResponseItem>,
) -> Option<&'static str> {
    for item in items.rev() {
        match item {
            ResponseItem::Message { role, .. } | ResponseItem::InputMessage { role, .. } => {
                match role.as_str() {
                    "assistant" => return None,
                    "user" => return Some("interrupted_prompt"),
                    _ => continue,
                }
            }
            ResponseItem::FunctionCall { .. }
            | ResponseItem::FunctionCallOutput { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::CustomToolCallOutput { .. }
            | ResponseItem::WebSearchCall { .. } => return Some("interrupted_turn"),
            ResponseItem::Reasoning { .. } => continue,
        }
    }
    None
}
