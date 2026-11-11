//! Original-document admission inside the existing read-only tool pipeline.
//! No text parsing, OCR, conversion, model request or external upload occurs.
use crate::canvas_artifact_service::CanvasArtifactService;
use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::workflow::documents::{
    invalid, DocumentReceipt, DocumentSelection, PreparedDocuments, DOCUMENT_READ_TOOL,
    MAX_DOCUMENT_BYTES,
};
use deepagent_tools::{
    PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolExecutionContext, ToolOutput,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) struct NativeDocument {
    pub name: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}
/// Run-local, one-use prepared snapshots. Bytes are deliberately not serializable.
pub struct AdmittedDocuments {
    pub(crate) receipt: PreparedDocuments,
    pub(crate) files: Vec<NativeDocument>,
    _permit: OwnedSemaphorePermit,
}
impl std::fmt::Debug for AdmittedDocuments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmittedDocuments")
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}
pub(crate) struct DocumentInputs {
    pending: Mutex<HashMap<String, Arc<AdmittedDocuments>>>,
    // Bound retained originals to one <=50MiB batch per run. Parallel nodes wait
    // under their existing tool deadline; this is memory backpressure, not a new run.
    capacity: Arc<Semaphore>,
}
impl Default for DocumentInputs {
    fn default() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            capacity: Arc::new(Semaphore::new(1)),
        }
    }
}
impl DocumentInputs {
    pub(crate) fn take(&self, receipt: &PreparedDocuments) -> Result<Arc<AdmittedDocuments>> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| invalid("document admission lock unavailable"))?;
        if pending
            .get(&receipt.request_id)
            .map_or(true, |stored| stored.receipt != *receipt)
        {
            return Err(invalid(
                "document admission is unknown, consumed or belongs to another run",
            ));
        }
        pending
            .remove(&receipt.request_id)
            .ok_or_else(|| invalid("document admission disappeared"))
    }
}

pub(crate) struct DocumentReadTool {
    artifacts: Option<Arc<CanvasArtifactService>>,
    workspace_id: Option<String>,
    inputs: Arc<DocumentInputs>,
}
impl DocumentReadTool {
    pub(crate) fn new(
        artifacts: Option<Arc<CanvasArtifactService>>,
        workspace_id: Option<String>,
    ) -> Self {
        Self {
            artifacts,
            workspace_id,
            inputs: Arc::new(DocumentInputs::default()),
        }
    }
    pub(crate) fn with_inputs(mut self, inputs: Arc<DocumentInputs>) -> Self {
        self.inputs = inputs;
        self
    }
    async fn read(
        &self,
        selection: DocumentSelection,
        context: ToolExecutionContext,
    ) -> Result<PreparedDocuments> {
        selection.validate()?;
        if selection.documents.is_empty() {
            return Err(invalid("document admission requires a non-empty selection"));
        }
        let artifacts = self
            .artifacts
            .as_ref()
            .ok_or_else(|| invalid("canvas document storage is unavailable"))?;
        let began = Instant::now();
        let check = || {
            if context.is_cancelled() || began.elapsed() >= context.timeout() {
                Err(CoreError::other(
                    "document admission cancelled or timed out",
                ))
            } else {
                Ok(())
            }
        };
        check()?;
        let permit = tokio::select! {
            _=context.cancelled()=>return Err(CoreError::other("document admission cancelled")),
            permit=tokio::time::timeout(context.timeout(),self.inputs.capacity.clone().acquire_owned())=>permit.map_err(|_|CoreError::other("document admission timed out"))?.map_err(|_|invalid("document admission closed"))?,
        };
        let mut receipts = Vec::new();
        let mut files = Vec::new();
        for (index, source) in selection.documents.iter().enumerate() {
            check()?;
            let service = artifacts.clone();
            let reference = source.reference.clone();
            let workspace = self.workspace_id.clone();
            let (record, bytes) = tokio::task::spawn_blocking(move || {
                service.read_document_snapshot(&reference, workspace.as_deref(), MAX_DOCUMENT_BYTES)
            })
            .await
            .map_err(|_| invalid("document snapshot task failed"))??;
            check()?;
            let format = Path::new(&record.path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let mime = match format.as_str() {
                "pdf" => "application/pdf",
                "txt" => "text/plain",
                "md" => "text/markdown",
                "csv" => "text/csv",
                "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "pptx" => {
                    "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                }
                _ => return Err(invalid("unsupported original document format")),
            };
            let leaf = source.name.rsplit(['/', '\\']).next().unwrap_or("");
            let name = if leaf.is_empty() {
                format!("attachment-{}.{}", index + 1, format)
            } else {
                leaf.to_owned()
            };
            files.push(NativeDocument { name, mime, bytes });
            receipts.push(DocumentReceipt {
                reference: source.reference.clone(),
                name: source.name.clone(),
                digest: record
                    .digest
                    .ok_or_else(|| invalid("document has no integrity digest"))?,
                format,
                byte_size: record.byte_size as u64,
            });
        }
        let receipt = PreparedDocuments {
            version: 1,
            request_id: format!("documents_{}", uuid::Uuid::new_v4()),
            documents: receipts,
        };
        receipt.validate(&selection)?;
        check()?;
        self.inputs
            .pending
            .lock()
            .map_err(|_| invalid("document admission lock unavailable"))?
            .insert(
                receipt.request_id.clone(),
                Arc::new(AdmittedDocuments {
                    receipt: receipt.clone(),
                    files,
                    _permit: permit,
                }),
            );
        Ok(receipt)
    }
}
#[async_trait::async_trait]
impl Tool for DocumentReadTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: DOCUMENT_READ_TOOL.into(),
            description: "Admit explicitly selected original document artifacts for one canvas model request; no text extraction.".into(),
            parameters: json!({
                "type": "object", "additionalProperties": false,
                "properties": {
                    "documents": {
                        "type": "array", "maxItems": 5,
                        "items": {
                            "type": "object", "additionalProperties": false,
                            "properties": {
                                "reference": {"type": "string"},
                                "name": {"type": "string"}
                            },
                            "required": ["reference"]
                        }
                    }
                },
                "required": ["documents"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }
    async fn invoke(&self, arguments: Value) -> Result<ToolOutput> {
        self.invoke_with_context(arguments, ToolExecutionContext::default())
            .await
    }
    async fn invoke_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let selection: DocumentSelection =
            serde_json::from_value(arguments).map_err(|_| invalid("invalid document selection"))?;
        if context.is_cancelled() {
            return Err(CoreError::other("document admission cancelled"));
        }
        Ok(ToolOutput::success(serde_json::to_value(
            self.read(selection, context).await?,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn h179_original_admission_is_scoped_one_use_bounded_and_cancel_safe() {
        use crate::canvas_artifact_service::CanvasArtifactImport;
        use base64::Engine;
        use std::time::Duration;
        let root = tempfile::tempdir().unwrap();
        let artifacts = Arc::new(
            CanvasArtifactService::new(
                root.path(),
                Arc::new(deepagent_persistence::Database::open_in_memory().unwrap()),
            )
            .unwrap(),
        );
        let artifact = artifacts
            .import(&CanvasArtifactImport {
                kind: "document".into(),
                data_url: Some(format!(
                    "data:application/pdf;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(b"%PDF-H179_ORIGINAL_SECRET")
                )),
                local_path: None,
                file_name: Some("original.pdf".into()),
                media_type: None,
                workspace_id: Some("run-space".into()),
            })
            .unwrap();
        let inputs = Arc::new(DocumentInputs::default());
        let reader = DocumentReadTool::new(Some(artifacts), Some("run-space".into()))
            .with_inputs(inputs.clone());
        let arguments = json!({"documents":[{"reference":artifact.uri,"name":"original.pdf"}]});
        let output = reader.invoke(arguments.clone()).await.unwrap();
        assert!(!output.value.to_string().contains("H179_ORIGINAL_SECRET"));
        let receipt: PreparedDocuments = serde_json::from_value(output.value).unwrap();
        assert!(DocumentInputs::default().take(&receipt).is_err());
        let mut forged = receipt.clone();
        forged.documents[0].name = "different.pdf".into();
        assert!(inputs.take(&forged).is_err());
        let held = inputs.take(&receipt).unwrap();
        assert!(inputs.take(&receipt).is_err());
        assert_eq!(held.files[0].bytes, b"%PDF-H179_ORIGINAL_SECRET");
        assert!(!format!("{held:?}").contains("H179_ORIGINAL_SECRET"));
        std::fs::write(&artifact.path, b"changed after admission").unwrap();
        assert_eq!(
            held.files[0].bytes, b"%PDF-H179_ORIGINAL_SECRET",
            "one request uses exactly the admitted immutable snapshot"
        );
        let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(50));
        assert!(reader
            .invoke_with_context(arguments.clone(), context)
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        let context = ToolExecutionContext::default();
        let cancel = context.cancellation_token();
        let pending = reader.invoke_with_context(arguments.clone(), context);
        tokio::pin!(pending);
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(10))=>{},_= &mut pending=>panic!("admission must wait for memory capacity")};
        cancel.cancel();
        assert!(pending.await.unwrap_err().to_string().contains("cancelled"));
        drop(held);
        assert_eq!(
            inputs.capacity.available_permits(),
            1,
            "the admitted request/drop releases its memory reservation"
        );
        assert!(
            reader.invoke(arguments).await.is_err(),
            "new admissions must recheck changed artifacts"
        );
        assert_eq!(inputs.capacity.available_permits(), 1);
    }

    #[tokio::test]
    async fn h179_document_read_requires_the_existing_read_permission_before_materialization() {
        let mut registry = deepagent_tools::ToolRegistry::new();
        registry
            .register(Arc::new(DocumentReadTool::new(None, None)))
            .unwrap();
        let pipeline = deepagent_runtime::ToolExecutionPipeline::new(
            &registry,
            deepagent_core::id::SessionId::new(),
            PermissionSet::empty(),
        );
        let result = pipeline
            .execute(deepagent_tools::ToolInvocation::new(
                DOCUMENT_READ_TOOL,
                json!({"documents":[{"reference":"artifact://synthetic","name":"one.txt"}]}),
            ))
            .await
            .unwrap();
        assert!(!result.output.ok);
        assert_eq!(
            result.stage,
            deepagent_runtime::ToolPipelineStage::Permission
        );
        assert!(!result
            .output
            .value
            .to_string()
            .contains("storage is unavailable"));
    }
    #[tokio::test]
    async fn h179_pre_cancelled_document_admission_does_not_touch_storage() {
        let context = ToolExecutionContext::new(Arc::new(std::sync::atomic::AtomicBool::new(true)));
        let error = DocumentReadTool::new(None, None)
            .invoke_with_context(
                json!({"documents":[{"reference":"artifact://synthetic","name":"one.txt"}]}),
                context,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
        assert!(!error.to_string().contains("storage is unavailable"));
    }
}
