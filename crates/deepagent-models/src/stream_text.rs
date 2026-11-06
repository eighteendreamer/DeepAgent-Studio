//! Per-item text assembly inside the Responses accumulator, never durable state.

use std::collections::{BTreeMap, HashSet};

use deepagent_core::error::{CoreError, Result};
use deepagent_core::response_item::ResponseOutputItem;
use serde_json::Value;

#[derive(Debug, Default)]
pub(super) struct TextItems(Vec<TextItem>);

#[derive(Debug)]
struct TextItem {
    id: Option<String>,
    output_index: Option<u64>,
    slot: usize,
    parts: BTreeMap<u64, String>,
    completed: bool,
}

impl TextItems {
    fn index(
        &mut self,
        id: Option<&str>,
        output_index: Option<u64>,
        output: &mut Vec<ResponseOutputItem>,
    ) -> Result<usize> {
        let mut candidates: Vec<_> = self
            .0
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                id.is_some_and(|id| item.id.as_deref() == Some(id))
                    || output_index.is_some_and(|index| item.output_index == Some(index))
            })
            .map(|(index, _)| index)
            .collect();
        if candidates.is_empty() {
            // Older transports omit item identity. Adopt only an unambiguous
            // open anonymous stream, not a completed item with matching text.
            candidates = self
                .0
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    (!item.completed
                        && ((id.is_none() && output_index.is_none())
                            || (item.id.is_none() && item.output_index.is_none())))
                        || (id.is_none()
                            && item.id.is_none()
                            && item.output_index.is_none()
                            && output_index == Some(item.slot as u64))
                })
                .map(|(index, _)| index)
                .collect();
        }
        if candidates.len() > 1 {
            return Err(identity_error("ambiguous text item"));
        }
        if let Some(&index) = candidates.first() {
            let item = &mut self.0[index];
            if (id.is_some() && item.id.is_some() && item.id.as_deref() != id)
                || (output_index.is_some()
                    && item.output_index.is_some()
                    && item.output_index != output_index)
            {
                return Err(identity_error("conflicting text item identity"));
            }
            item.id = item.id.take().or_else(|| id.map(str::to_owned));
            item.output_index = item.output_index.or(output_index);
            return Ok(index);
        }
        let index = self.0.len();
        let slot = output.len();
        output.push(ResponseOutputItem::Message {
            role: "assistant".into(),
            content: String::new(),
        });
        self.0.push(TextItem {
            id: id.map(str::to_owned),
            output_index,
            slot,
            parts: BTreeMap::new(),
            completed: false,
        });
        Ok(index)
    }

    pub(super) fn start(
        &mut self,
        item: &Value,
        output_index: Option<u64>,
        output: &mut Vec<ResponseOutputItem>,
    ) -> Result<()> {
        message_role(item)?;
        self.index(item.get("id").and_then(Value::as_str), output_index, output)?;
        Ok(())
    }

    pub(super) fn delta(
        &mut self,
        event: &Value,
        text: &str,
        output: &mut Vec<ResponseOutputItem>,
    ) -> Result<()> {
        let index = self.index(
            event.get("item_id").and_then(Value::as_str),
            event.get("output_index").and_then(Value::as_u64),
            output,
        )?;
        let item = &mut self.0[index];
        if item.completed {
            return Err(identity_error("text delta after item completion"));
        }
        item.parts
            .entry(
                event
                    .get("content_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            )
            .or_default()
            .push_str(text);
        Ok(())
    }

    pub(super) fn complete(
        &mut self,
        value: &Value,
        output_index: Option<u64>,
        output: &mut Vec<ResponseOutputItem>,
    ) -> Result<()> {
        let role = message_role(value)?;
        let index = self.index(
            value.get("id").and_then(Value::as_str),
            output_index,
            output,
        )?;
        let item = &mut self.0[index];
        output[item.slot] = ResponseOutputItem::Message {
            role: role.to_owned(),
            // Explicit empty content is authoritative. An omitted content
            // field may only recover this item's own fragments, never a sibling.
            content: super::output_text_from_message_item(value)
                .unwrap_or_else(|| item.parts.values().cloned().collect()),
        };
        item.completed = true;
        Ok(())
    }

    pub(super) fn finish(
        self,
        mut output: Vec<ResponseOutputItem>,
    ) -> (Vec<ResponseOutputItem>, Option<usize>) {
        let mut unused = HashSet::new();
        let mut first_fallback = None;
        for item in self.0 {
            if item.completed {
                continue;
            }
            if item.parts.is_empty() {
                unused.insert(item.slot);
            } else if let ResponseOutputItem::Message { content, .. } = &mut output[item.slot] {
                *content = item.parts.into_values().collect();
                first_fallback.get_or_insert(item.slot);
            }
        }
        let first_fallback = first_fallback.map(|slot| {
            slot - unused
                .iter()
                .filter(|&&unused_slot| unused_slot < slot)
                .count()
        });
        let output = output
            .into_iter()
            .enumerate()
            .filter_map(|(index, item)| (!unused.contains(&index)).then_some(item))
            .collect();
        (output, first_fallback)
    }
}

fn message_role(value: &Value) -> Result<&str> {
    value
        .get("role")
        .and_then(Value::as_str)
        .filter(|role| *role == "assistant")
        .ok_or_else(|| {
            // Provider values may contain private text. Keep only the field
            // path and expected contract in the diagnostic.
            CoreError::provider(
                None,
                Some("malformed_response_message".into()),
                "Responses output message.role must be the string assistant",
            )
        })
}

fn identity_error(detail: &str) -> CoreError {
    CoreError::provider(
        None,
        Some("message_item_identity_conflict".into()),
        format!("Responses {detail}"),
    )
}
