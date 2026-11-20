//! Debug-only evidence from the finalized Responses body. Never write request
//! data or fingerprints to disk; keep one previous request in process memory.

use deepagent_core::id::SessionId;
use deepagent_core::response_item::ResponseInputItem;
use deepagent_core::runtime_snapshot::RuntimeSnapshot;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

#[derive(PartialEq, Eq)]
struct Part {
    digest: [u8; 32],
    bytes: usize,
}

impl Part {
    fn new(value: &Value) -> Option<Self> {
        let bytes = serde_json::to_vec(value).ok()?;
        Some(Self {
            digest: Sha256::digest(&bytes).into(),
            bytes: bytes.len(),
        })
    }
}

struct Frame {
    route: Part,
    instructions: Part,
    tools: Vec<Part>,
    input: Vec<Part>,
    snapshots: Vec<usize>,
    sent_cache_key: bool,
}

impl Frame {
    fn new(url: &str, body: &str, canonical: &[ResponseInputItem]) -> Option<Self> {
        let wire: Value = serde_json::from_str(body).ok()?;
        let parts = |items: &[Value]| items.iter().map(Part::new).collect::<Option<Vec<_>>>();
        let input = parts(wire["input"].as_array()?)?;
        if input.len() != canonical.len() {
            return None;
        }
        Some(Self {
            route: Part::new(&json!([
                url,
                wire["model"],
                wire["reasoning"],
                wire["tool_choice"],
                wire["parallel_tool_calls"],
                wire["prompt_cache_key"],
            ]))?,
            instructions: Part::new(&wire["instructions"])?,
            tools: parts(wire["tools"].as_array().map_or(&[], Vec::as_slice))?,
            input,
            snapshots: canonical
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    RuntimeSnapshot::has_envelope_marker(item).then_some(index)
                })
                .collect(),
            sent_cache_key: wire["prompt_cache_key"].is_string(),
        })
    }

    fn compare(&self, previous: Option<&Self>) -> Value {
        let common =
            |old: &[Part], new: &[Part]| old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let common_tools = previous.map(|old| common(&old.tools, &self.tools));
        let common_input = previous.map(|old| common(&old.input, &self.input));
        let sum = |parts: &[Part]| parts.iter().map(|part| part.bytes).sum::<usize>();
        json!({
            "instructions_changed": previous.map(|old| old.instructions != self.instructions),
            "route_fields_changed": previous.map(|old| old.route != self.route),
            "tools": self.tools.len(),
            "previous_tools": previous.map(|old| old.tools.len()),
            "common_tools": common_tools,
            "input_items": self.input.len(),
            "previous_input_items": previous.map(|old| old.input.len()),
            "common_input_items": common_input,
            "input_suffix_bytes": sum(&self.input[common_input.unwrap_or(0)..]),
            "snapshot_bytes": self.snapshots.iter().map(|&index| self.input[index].bytes).collect::<Vec<_>>(),
            "snapshots_in_input_suffix": self.snapshots.iter().filter(|&&index| index >= common_input.unwrap_or(0)).count(),
            "sent_cache_key": self.sent_cache_key,
        })
    }
}

type Previous = (SessionId, usize, Frame);
static PREVIOUS: Mutex<Option<Previous>> = Mutex::new(None);

pub(super) fn record(
    session: Option<SessionId>,
    url: &str,
    body: &str,
    canonical: &[ResponseInputItem],
) {
    let (Some(session), Some(path)) = (session, std::env::var_os("DEEPAGENT_CACHE_AUDIT_FILE"))
    else {
        return;
    };
    let Some(frame) = Frame::new(url, body, canonical) else {
        return;
    };
    let mut previous = PREVIOUS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior = previous.as_ref().filter(|(sid, _, _)| *sid == session);
    let ordinal = prior.map_or(1, |(_, count, _)| count + 1);
    let mut evidence = frame.compare(prior.map(|(_, _, frame)| frame));
    evidence["request"] = json!(ordinal);
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{evidence}"));
    if result.is_err() {
        tracing::warn!("could not write cache audit counters");
    }
    *previous = Some((session, ordinal, frame));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(tools: &[&str], input: &[ResponseInputItem]) -> Frame {
        Frame::new("fixture", &json!({"model":"fixture", "instructions":"private rules", "tools":tools, "input":input}).to_string(), input).unwrap()
    }

    #[test]
    fn cache_audit_detects_append_without_losing_original_prefix() {
        let user = ResponseInputItem::Message {
            role: "user".into(),
            content: "private user text".into(),
        };
        let runtime = RuntimeSnapshot {
            version: 1,
            series: 1,
            revision: 1,
            stable_rules_sha256: "a".repeat(64),
            context: "private env".into(),
        }
        .to_item();
        let before = frame(&["private tool description"], std::slice::from_ref(&user));
        let after = frame(&["private tool description"], &[user, runtime]);
        let diff = after.compare(Some(&before));
        assert_eq!(diff["common_input_items"], 1);
        assert_eq!(diff["common_tools"], 1);
        assert_eq!(diff["instructions_changed"], false);
        assert_eq!(diff["snapshots_in_input_suffix"], 1);
        assert!(!diff.to_string().contains("private"));
    }

    #[test]
    fn cache_audit_locates_reordering_even_when_counts_match() {
        let first = ResponseInputItem::Message {
            role: "user".into(),
            content: "one".into(),
        };
        let second = ResponseInputItem::Message {
            role: "user".into(),
            content: "two".into(),
        };
        let before = frame(&["A", "B"], &[first.clone(), second.clone()]);
        let after = frame(&["B", "A"], &[second, first]);
        let diff = after.compare(Some(&before));
        assert_eq!(diff["common_tools"], 0);
        assert_eq!(diff["common_input_items"], 0);
    }
}
