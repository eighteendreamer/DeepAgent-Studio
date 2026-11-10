//! Native discovery item assembly only. Execution remains in the existing
//! tool pipeline; server-supplied schemas are data, never host registrations.
use super::*;
use deepagent_core::response_item::ToolSearchExecution;

#[derive(Debug, Default)]
pub(super) struct ToolSearchItems {
    records: Vec<Record>,
}

#[derive(Debug)]
struct Record {
    index: Option<u64>,
    item: ResponseOutputItem,
    completed: bool,
}

fn identity(item: &ResponseOutputItem) -> (&Option<String>, &Option<String>, bool) {
    match item {
        ResponseOutputItem::ToolSearchCall { id, call_id, .. } => (id, call_id, false),
        ResponseOutputItem::ToolSearchOutput { id, call_id, .. } => (id, call_id, true),
        _ => unreachable!("native discovery records contain only native discovery items"),
    }
}

impl ToolSearchItems {
    pub(super) fn completed_at(&self, index: u64) -> bool {
        self.records
            .iter()
            .any(|record| record.index == Some(index) && record.completed)
    }

    pub(super) fn conflicts(
        &self,
        id: Option<&str>,
        call_id: Option<&str>,
        index: Option<u64>,
    ) -> bool {
        self.records.iter().any(|record| {
            let (saved_id, saved_call, _) = identity(&record.item);
            id.is_some_and(|id| saved_id.as_deref() == Some(id))
                || call_id.is_some_and(|id| saved_call.as_deref() == Some(id))
                || index.is_some_and(|index| record.index == Some(index))
        })
    }

    pub(super) fn record(
        &mut self,
        value: &serde_json::Value,
        index: Option<u64>,
        complete: bool,
        output: &mut Vec<ResponseOutputItem>,
    ) -> Result<()> {
        let item: ResponseOutputItem = serde_json::from_value(value.clone())
            .map_err(|_| CoreError::Serialization("malformed native tool discovery item".into()))?;
        let (id, call_id, is_output) = identity(&item);
        let execution = match &item {
            ResponseOutputItem::ToolSearchCall { execution, .. }
            | ResponseOutputItem::ToolSearchOutput { execution, .. } => *execution,
            _ => unreachable!(),
        };
        if id.as_ref().is_some_and(|id| id.trim().is_empty())
            || call_id.as_ref().is_some_and(|id| id.trim().is_empty())
            || execution == ToolSearchExecution::Client && call_id.is_none()
            || execution == ToolSearchExecution::Server && id.is_none() && index.is_none()
        {
            return Err(CoreError::Serialization(
                "invalid native tool discovery identity".into(),
            ));
        }
        if complete {
            if let ResponseOutputItem::ToolSearchCall { arguments, .. } = &item {
                if execution == ToolSearchExecution::Client && !arguments.is_object() {
                    return Err(CoreError::Serialization(
                        "native client discovery arguments must be an object".into(),
                    ));
                }
            }
        }
        let matches: Vec<_> = self
            .records
            .iter()
            .enumerate()
            .filter_map(|(position, record)| {
                let (saved_id, saved_call, saved_output) = identity(&record.item);
                (id.as_ref().is_some_and(|id| saved_id.as_ref() == Some(id))
                    || index.is_some_and(|index| record.index == Some(index))
                    || is_output == saved_output
                        && call_id
                            .as_ref()
                            .is_some_and(|id| saved_call.as_ref() == Some(id)))
                .then_some(position)
            })
            .collect();
        if matches.len() > 1 {
            return Err(CoreError::Serialization(
                "conflicting native discovery item identities".into(),
            ));
        }
        if let Some(&position) = matches.first() {
            let record = &mut self.records[position];
            let (saved_id, saved_call, saved_output) = identity(&record.item);
            let saved_execution = match &record.item {
                ResponseOutputItem::ToolSearchCall { execution, .. }
                | ResponseOutputItem::ToolSearchOutput { execution, .. } => *execution,
                _ => unreachable!(),
            };
            if saved_id != id
                || saved_call != call_id
                || saved_output != is_output
                || saved_execution != execution
                || record.index.zip(index).is_some_and(|(a, b)| a != b)
                || record.completed && record.item != item
            {
                return Err(CoreError::Serialization(
                    "conflicting native discovery item identity or completed content".into(),
                ));
            }
            record.index = record.index.or(index);
            record.item = item;
            if complete && !record.completed {
                record.completed = true;
                output.push(record.item.clone());
            }
            return Ok(());
        }
        if complete {
            output.push(item.clone());
        }
        self.records.push(Record {
            index,
            item,
            completed: complete,
        });
        Ok(())
    }
}
