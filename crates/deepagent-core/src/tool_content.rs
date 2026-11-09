//! Durable tool-result content. Images are references, never inline payloads.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::message::{Message, MessageAttachment};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultPart {
    Text { text: String },
    Image { attachment: MessageAttachment },
}

/// Old textual histories keep their exact JSON string representation. A typed
/// result preserves every text/image occurrence in order across event replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultBody {
    Text(String),
    Parts(Vec<ToolResultPart>),
}

impl ToolResultBody {
    pub fn from_message(message: &Message) -> Self {
        if message.attachments.is_empty() {
            return message.content.clone().into();
        }
        let mut parts = vec![ToolResultPart::Text {
            text: message.content.clone(),
        }];
        parts.extend(
            message
                .attachments
                .iter()
                .cloned()
                .map(|attachment| ToolResultPart::Image { attachment }),
        );
        Self::Parts(parts)
    }

    /// The common live/persisted result envelope. `parts` already contains
    /// trusted host image references; arbitrary tool JSON cannot mint them.
    pub fn with_parts(text: String, parts: &[ToolResultPart]) -> Self {
        if parts.is_empty() {
            return text.into();
        }
        let mut content = Vec::with_capacity(parts.len() + 1);
        content.push(ToolResultPart::Text { text });
        content.extend_from_slice(parts);
        Self::Parts(content)
    }

    /// Explicit compatibility projection for text-only consumers (UI, snip,
    /// diagnostics). Provider and persistence paths must retain `Parts`.
    pub fn text(&self) -> Cow<'_, str> {
        match self {
            Self::Text(text) => Cow::Borrowed(text),
            Self::Parts(parts) => Cow::Owned(
                parts
                    .iter()
                    .filter_map(|part| match part {
                        ToolResultPart::Text { text } => Some(text.as_str()),
                        ToolResultPart::Image { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        }
    }

    pub fn attachments(&self) -> impl Iterator<Item = &MessageAttachment> {
        let parts = match self {
            Self::Text(_) => &[][..],
            Self::Parts(parts) => parts.as_slice(),
        };
        parts.iter().filter_map(|part| match part {
            ToolResultPart::Image { attachment } => Some(attachment),
            ToolResultPart::Text { .. } => None,
        })
    }
}

impl From<String> for ToolResultBody {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for ToolResultBody {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_strings_and_ordered_references_roundtrip_without_inline_data() {
        let legacy: ToolResultBody = serde_json::from_str("\"old output\"").unwrap();
        assert_eq!(serde_json::to_string(&legacy).unwrap(), "\"old output\"");
        let body = ToolResultBody::with_parts(
            "status".into(),
            &[
                ToolResultPart::Image {
                    attachment: MessageAttachment {
                        id: "persisted".into(),
                        kind: "image".into(),
                        media_type: Some("image/png".into()),
                        path: Some("original.png".into()),
                        sha256: Some("a".repeat(64)),
                    },
                },
                ToolResultPart::Text {
                    text: "after".into(),
                },
            ],
        );
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(serde_json::from_str::<ToolResultBody>(&json).unwrap(), body);
        assert_eq!(body.text(), "status\nafter");
        assert_eq!(body.attachments().count(), 1);
        assert!(!json.contains("base64"));
    }
}
