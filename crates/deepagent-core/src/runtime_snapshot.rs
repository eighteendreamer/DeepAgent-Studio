//! Local, versioned host-context contract carried by ordinary system items.
//! This is not a provider replacement API or a second event/history store.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::response_item::ResponseItem;

const MARKER: &str = "[DeepAgent runtime context snapshot]\n";
const PREFIX: &str = "[DeepAgent runtime context snapshot]\n\
This complete snapshot replaces only earlier DeepAgent runtime context snapshots. \
It does not replace stable system rules. Historical snapshots describe past state; \
only the latest snapshot describes current environment, permissions and catalogs.\n";

const SECTION_PREFIX: &str = "[DeepAgent runtime context snapshot]\n\
Apply these named environment sections in order: full replaces the prior baseline; \
delta replaces only the named sections, and null removes a section. \
These updates never replace stable system rules.\n";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionUpdate {
    full: bool,
    sections: BTreeMap<String, Option<String>>,
}

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
                "{}{}",
                if self.version == 2 {
                    SECTION_PREFIX
                } else {
                    PREFIX
                },
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
        let (version, body) = if let Some(body) = content.strip_prefix(PREFIX) {
            (1, body)
        } else {
            (2, content.strip_prefix(SECTION_PREFIX)?)
        };
        let snapshot: Self = serde_json::from_str(body).ok()?;
        (snapshot.version == version
            && (version == 1 || snapshot.section_update().is_some())
            && snapshot.series > 0
            && snapshot.revision > 0
            && snapshot.stable_rules_sha256.len() == 64
            && snapshot
                .stable_rules_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit()))
        .then_some(snapshot)
    }

    fn section_update(&self) -> Option<SectionUpdate> {
        (self.version == 2).then_some(())?;
        serde_json::from_str(&self.context).ok()
    }

    pub fn with_sections(mut self, full: bool, sections: BTreeMap<String, Option<String>>) -> Self {
        self.version = 2;
        self.context = serde_json::to_string(&SectionUpdate { full, sections })
            .expect("section update strings");
        self
    }
}

pub fn latest_snapshot(items: &[ResponseItem]) -> Option<(&ResponseItem, RuntimeSnapshot)> {
    items
        .iter()
        .rev()
        .find_map(|item| RuntimeSnapshot::from_item(item).map(|s| (item, s)))
}

/// Rebuild the baseline from the actual retained history, including after reloading a session.
pub fn latest_sections(
    items: &[ResponseItem],
) -> Option<(RuntimeSnapshot, BTreeMap<String, String>)> {
    let mut history = items.iter().rev().filter_map(RuntimeSnapshot::from_item);
    let latest = history.next()?;
    let mut updates = Vec::new();
    let mut expected = latest.revision;
    for snapshot in std::iter::once(latest.clone()).chain(history) {
        if snapshot.series != latest.series
            || snapshot.revision != expected
            || snapshot.stable_rules_sha256 != latest.stable_rules_sha256
        {
            return None;
        }
        let update = snapshot.section_update()?;
        let full = update.full;
        updates.push(update.sections);
        if full {
            let mut sections = BTreeMap::new();
            for changes in updates.into_iter().rev() {
                for (name, content) in changes {
                    if let Some(content) = content {
                        sections.insert(name, content);
                    } else {
                        sections.remove(&name);
                    }
                }
            }
            return Some((latest, sections));
        }
        expected = expected.checked_sub(1)?;
    }
    None
}

/// Compaction may remove the old snapshot's original position. Carry exactly
/// the latest full snapshot after the surviving tail, identically live/replay.
/// Never regenerate it from a summary or revive a superseded snapshot.
pub fn retain_latest_snapshot(source: &[ResponseItem], target: &mut Vec<ResponseItem>) {
    if let Some((snapshot, sections)) = latest_sections(source) {
        target.retain(|item| !RuntimeSnapshot::has_envelope_marker(item));
        target.push(
            snapshot
                .with_sections(
                    true,
                    sections.into_iter().map(|(k, v)| (k, Some(v))).collect(),
                )
                .to_item(),
        );
    } else if let Some((item, _)) =
        latest_snapshot(source).filter(|(_, snapshot)| snapshot.version == 1)
    {
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
