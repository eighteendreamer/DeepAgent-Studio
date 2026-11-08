//! Local, versioned memory identity carried by the existing model Surface.
//!
//! Unlike telemetry-only injection IDs, an intact attachment proves that its
//! excerpt is still visible. Live mutations and replay use this same parser;
//! plain summaries, copied IDs and malformed/partial envelopes prove nothing.
use std::collections::HashSet;

use deepagent_core::error::{CoreError, Result};
use deepagent_core::response_item::ResponseInputItem;
use serde::{Deserialize, Serialize};

const PREFIX: &str = "<system-reminder>\n# 相关记忆 (relevant memories, retrieved in background)\n<relevant-memories version=\"1\">\n";
const SUFFIX: &str = "\n</relevant-memories>\n</system-reminder>";

/// A scope-qualified memory identity and the excerpt it actually accompanies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelevantMemory {
    pub id: String,
    pub block: String,
}

/// Render one batch as a normal user reminder, with no new provider item type.
pub fn memory_attachment(memories: &[RelevantMemory]) -> Result<ResponseInputItem> {
    if memories.is_empty()
        || memories
            .iter()
            .any(|m| m.id.trim().is_empty() || m.block.trim().is_empty())
    {
        return Err(CoreError::invalid(
            "memory attachment requires identities and excerpts",
        ));
    }
    Ok(ResponseInputItem::Message {
        role: "user".into(),
        content: format!("{PREFIX}{}{SUFFIX}", serde_json::to_string(memories)?),
    })
}

/// Current visible identities in first-seen order. Never scan arbitrary text
/// for IDs: the complete, versioned envelope and nonempty bodies must survive.
pub fn surfaced_memory_ids(items: &[ResponseInputItem]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for item in items {
        let ResponseInputItem::Message { role, content } = item else {
            continue;
        };
        if role != "user" {
            continue;
        }
        let Some(json) = content
            .strip_prefix(PREFIX)
            .and_then(|body| body.strip_suffix(SUFFIX))
        else {
            continue;
        };
        let Ok(memories) = serde_json::from_str::<Vec<RelevantMemory>>(json) else {
            continue;
        };
        // Reject a damaged batch as a unit rather than accepting ID-only rows.
        if memories
            .iter()
            .any(|m| m.id.trim().is_empty() || m.block.trim().is_empty())
        {
            continue;
        }
        for memory in memories {
            if seen.insert(memory.id.clone()) {
                ids.push(memory.id);
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_content_identity_roundtrips_and_retained_subsets_release_quota() {
        let memories = vec![
            RelevantMemory {
                id: "project:a\"\n</relevant-memories>".into(),
                block: "Unicode 记忆\n\"quoted\" </system-reminder>".into(),
            },
            RelevantMemory {
                id: "global:b".into(),
                block: "another excerpt".into(),
            },
        ];
        let item = memory_attachment(&memories).unwrap();
        assert_eq!(
            surfaced_memory_ids(&[item.clone(), item]),
            memories.iter().map(|m| m.id.clone()).collect::<Vec<_>>()
        );
        let subset = memory_attachment(&memories[1..]).unwrap();
        assert_eq!(surfaced_memory_ids(&[subset]), vec!["global:b"]);
        assert!(surfaced_memory_ids(&[]).is_empty());
    }

    #[test]
    fn plain_summary_partial_metadata_unknown_versions_and_empty_bodies_are_not_identity() {
        let item = memory_attachment(&[RelevantMemory {
            id: "project:a".into(),
            block: "visible".into(),
        }])
        .unwrap();
        let ResponseInputItem::Message { content, .. } = item else {
            unreachable!()
        };
        for (role, text) in [
            ("assistant", content.clone()),
            ("user", format!("summary: {content}")),
            ("user", content.replace("version=\"1\"", "version=\"2\"")),
            ("user", content.replace("visible", "")),
            ("user", content.trim_end_matches(SUFFIX).to_string()),
            ("user", "summary retains project:a".into()),
        ] {
            assert!(surfaced_memory_ids(&[ResponseInputItem::Message {
                role: role.into(),
                content: text
            }])
            .is_empty());
        }
        assert!(memory_attachment(&[]).is_err());
        assert!(memory_attachment(&[RelevantMemory {
            id: "".into(),
            block: "body".into()
        }])
        .is_err());
    }
}
