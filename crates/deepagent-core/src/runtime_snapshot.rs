//! Local, versioned host-context contract carried by ordinary system items.
//! This is not a provider replacement API or a second event/history store.

use serde::{Deserialize, Serialize};

use crate::response_item::ResponseItem;

const MARKER: &str = "[DeepAgent runtime context snapshot]\n";
const PREFIX: &str = "[DeepAgent runtime context snapshot]\n\
This complete snapshot replaces only earlier DeepAgent runtime context snapshots. \
It does not replace stable system rules. Historical snapshots describe past state; \
only the latest snapshot describes current environment, permissions and catalogs.\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSnapshot {
    pub version: u32,
    pub series: u64,
    pub revision: u64,
    pub stable_rules_sha256: String,
    pub context: String,
}

impl RuntimeSnapshot {
    pub fn has_envelope_marker(item: &ResponseItem) -> bool {
        matches!(item, ResponseItem::Message { role, content } if role == "system" && content.starts_with(MARKER))
    }

    pub fn to_item(&self) -> ResponseItem {
        ResponseItem::Message {
            role: "system".into(),
            content: format!(
                "{PREFIX}{}",
                serde_json::to_string(self).expect("snapshot strings")
            ),
        }
    }

    /// Only the exact system envelope participates in snapshot handling. Quotes
    /// in user, assistant or tool content are not interpreted as host context.
    pub fn from_item(item: &ResponseItem) -> Option<Self> {
        let ResponseItem::Message { role, content } = item else {
            return None;
        };
        if role != "system" {
            return None;
        }
        let snapshot: Self = serde_json::from_str(content.strip_prefix(PREFIX)?).ok()?;
        (snapshot.version == 1
            && snapshot.series > 0
            && snapshot.revision > 0
            && snapshot.stable_rules_sha256.len() == 64
            && snapshot
                .stable_rules_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit()))
        .then_some(snapshot)
    }
}

pub fn latest_snapshot(items: &[ResponseItem]) -> Option<(&ResponseItem, RuntimeSnapshot)> {
    items
        .iter()
        .rev()
        .find_map(|item| RuntimeSnapshot::from_item(item).map(|s| (item, s)))
}

/// Compaction may remove the old snapshot's original position. Carry exactly
/// the latest full snapshot after the surviving tail, identically live/replay.
/// Never regenerate it from a summary or revive a superseded snapshot.
pub fn retain_latest_snapshot(source: &[ResponseItem], target: &mut Vec<ResponseItem>) {
    if let Some((item, _)) = latest_snapshot(source) {
        target.retain(|candidate| candidate != item);
        target.push(item.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(revision: u64) -> ResponseItem {
        RuntimeSnapshot {
            version: 1,
            series: 1,
            revision,
            stable_rules_sha256: "a".repeat(64),
            context: format!("state {revision}"),
        }
        .to_item()
    }

    #[test]
    fn h168_snapshot_roundtrip_requires_host_system_role_and_known_version() {
        let item = snapshot(1);
        assert_eq!(
            RuntimeSnapshot::from_item(&item).unwrap().context,
            "state 1"
        );
        let ResponseItem::Message { content, .. } = item else {
            unreachable!()
        };
        for role in ["user", "assistant", "tool"] {
            assert!(RuntimeSnapshot::from_item(&ResponseItem::Message {
                role: role.into(),
                content: content.clone(),
            })
            .is_none());
        }
        for content in [
            content.replace("\"version\":1", "\"version\":2"),
            format!("{PREFIX}{{broken"),
        ] {
            assert!(RuntimeSnapshot::from_item(&ResponseItem::Message {
                role: "system".into(),
                content
            })
            .is_none());
        }
    }

    #[test]
    fn h168_compaction_carries_latest_once_without_reviving_old_snapshot() {
        let source = vec![snapshot(1), snapshot(2)];
        let mut target = vec![];
        retain_latest_snapshot(&source, &mut target);
        retain_latest_snapshot(&source, &mut target);
        assert_eq!(target, vec![snapshot(2)]);
        assert_eq!(latest_snapshot(&target).unwrap().1.revision, 2);
    }
}
