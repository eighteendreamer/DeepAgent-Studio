//! Normalized user-input attachments.
//!
//! Claude Code's Prompt Submission layer accepts more than plain text: file
//! references, images, and pasted blobs. Before anything reaches the model we
//! normalize them into a uniform [`Attachment`] so the rest of the pipeline
//! (context assembly, prompt building) treats them consistently.

use serde::{Deserialize, Serialize};

/// The kind of an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    /// A reference to a workspace file (by relative path).
    File,
    /// A reference to a directory (by relative path).
    Directory,
    /// An image (path or data URI).
    Image,
    /// A pasted/inline text blob (e.g. a stack trace).
    Text,
}

impl AttachmentKind {
    /// Stable string label.
    pub const fn label(&self) -> &'static str {
        match self {
            AttachmentKind::File => "file",
            AttachmentKind::Directory => "directory",
            AttachmentKind::Image => "image",
            AttachmentKind::Text => "text",
        }
    }
}

/// A normalized attachment carried alongside the user's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    /// What kind of attachment this is.
    pub kind: AttachmentKind,
    /// The reference: a path for File/Directory/Image, or the inline content
    /// for Text.
    pub value: String,
    /// Optional human label (e.g. the original `#File` token).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Attachment {
    /// A workspace file reference.
    pub fn file(path: impl Into<String>) -> Self {
        Self {
            kind: AttachmentKind::File,
            value: path.into(),
            label: None,
        }
    }

    /// A workspace directory reference.
    pub fn directory(path: impl Into<String>) -> Self {
        Self {
            kind: AttachmentKind::Directory,
            value: path.into(),
            label: None,
        }
    }

    /// An image reference.
    pub fn image(path: impl Into<String>) -> Self {
        Self {
            kind: AttachmentKind::Image,
            value: path.into(),
            label: None,
        }
    }

    /// An inline text blob.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            kind: AttachmentKind::Text,
            value: content.into(),
            label: None,
        }
    }

    /// Attach a human label (builder-style).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// Extract `#File`/`#Folder`-style mention tokens from `text`, returning the
/// referenced paths. A mention is `#` followed by a non-whitespace run; the
/// leading `File:`/`Folder:` qualifier (if present) is stripped.
///
/// This mirrors Kiro/Claude Code's `#File`/`#Folder` chat context syntax and is
/// used by the router to lift inline references into structured attachments.
pub fn extract_mentions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in text.split_whitespace() {
        if let Some(rest) = token.strip_prefix('#') {
            let rest = rest
                .strip_prefix("File:")
                .or_else(|| rest.strip_prefix("Folder:"))
                .unwrap_or(rest);
            if !rest.is_empty() {
                out.push(rest.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_set_kind() {
        assert_eq!(Attachment::file("a.rs").kind, AttachmentKind::File);
        assert_eq!(Attachment::image("x.png").kind, AttachmentKind::Image);
        assert_eq!(Attachment::text("blob").kind, AttachmentKind::Text);
    }

    #[test]
    fn label_builder() {
        let a = Attachment::file("a.rs").with_label("#File:a.rs");
        assert_eq!(a.label.as_deref(), Some("#File:a.rs"));
    }

    #[test]
    fn extract_plain_mentions() {
        let m = extract_mentions("look at #src/main.rs and #Cargo.toml please");
        assert_eq!(m, vec!["src/main.rs", "Cargo.toml"]);
    }

    #[test]
    fn extract_qualified_mentions() {
        let m = extract_mentions("check #File:src/lib.rs in #Folder:tests");
        assert_eq!(m, vec!["src/lib.rs", "tests"]);
    }

    #[test]
    fn no_mentions() {
        assert!(extract_mentions("just some plain text").is_empty());
    }

    #[test]
    fn serde_roundtrip() {
        let a = Attachment::file("a.rs").with_label("#a");
        let json = serde_json::to_string(&a).unwrap();
        let back: Attachment = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }
}
