//! Selected original documents. Only opaque references and admission receipts
//! cross the runtime seam; bytes stay in the run-scoped gateway, never prompts.
use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const DOCUMENT_READ_TOOL: &str = "canvas_document_read";
pub const MAX_DOCUMENTS: usize = 5;
pub const MAX_DOCUMENT_BYTES: u64 = 10 * 1024 * 1024;

pub fn invalid(message: &str) -> CoreError {
    CoreError::invalid(format!("DocumentInputInvalid: {message}"))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentReference {
    pub reference: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSelection {
    pub documents: Vec<DocumentReference>,
}

impl DocumentSelection {
    pub fn from_config(
        config: &Map<String, Value>,
        outputs: &BTreeMap<String, Value>,
    ) -> Result<Self> {
        let documents = config.get("llmDocuments").cloned().unwrap_or(json!([]));
        let mut selection: Self = serde_json::from_value(json!({"documents":documents}))
            .map_err(|_| invalid("attachments must be an array of document references"))?;
        for document in &mut selection.documents {
            document.reference =
                super::values::resolve(&Value::String(document.reference.clone()), outputs)?
                    .as_str()
                    .ok_or_else(|| invalid("file binding must resolve to one artifact reference"))?
                    .to_owned();
        }
        selection.validate()?;
        Ok(selection)
    }
    pub fn validate(&self) -> Result<()> {
        if self.documents.len() > MAX_DOCUMENTS {
            return Err(invalid("at most five selected documents are allowed"));
        }
        let mut unique = BTreeSet::new();
        for document in &self.documents {
            let id = document.reference.strip_prefix("artifact://").unwrap_or("");
            if id.is_empty()
                || id.len() > 200
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                || document.name.chars().count() > 255
                || document.name.contains('\0')
                || !unique.insert(&document.reference)
            {
                return Err(invalid(
                    "invalid or repeated artifact reference or document name",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentReceipt {
    pub reference: String,
    pub name: String,
    pub digest: String,
    pub format: String,
    pub byte_size: u64,
}

/// One-use admission handle scoped to the same gateway that read the originals
/// through the normal tool pipeline. Cannot authorize a new path or URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedDocuments {
    pub version: u32,
    pub request_id: String,
    pub documents: Vec<DocumentReceipt>,
}
impl PreparedDocuments {
    pub fn validate(&self, selection: &DocumentSelection) -> Result<()> {
        selection.validate()?;
        if self.version != 1
            || self.documents.len() != selection.documents.len()
            || !self.request_id.starts_with("documents_")
            || self.request_id.len() > 100
        {
            return Err(invalid(
                "incomplete or incompatible document admission response",
            ));
        }
        for (document, expected) in self.documents.iter().zip(&selection.documents) {
            let digest = document.digest.strip_prefix("sha256:").unwrap_or("");
            if document.reference != expected.reference
                || document.name != expected.name
                || document.byte_size == 0
                || document.byte_size > MAX_DOCUMENT_BYTES
                || digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !matches!(
                    document.format.as_str(),
                    "txt" | "md" | "csv" | "docx" | "xlsx" | "pptx" | "pdf"
                )
            {
                return Err(invalid(
                    "document admission identity or format validation failed",
                ));
            }
        }
        Ok(())
    }
    pub fn receipts(&self) -> Value {
        json!(self.documents)
    }
}
