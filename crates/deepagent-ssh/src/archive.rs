//! Bounded extraction through a reviewed POSIX helper; never invoke tar/unzip on
//! untrusted entry names. Preview and execution use the same snapshot contract.
use crate::{FileTransferError, SshService};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveSpec {
    pub connection_id: String,
    pub archive_path: String,
    pub destination_path: String,
    pub conflict: crate::FileConflictPolicy,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePreview {
    pub version: u8,
    pub connection_generation: String,
    pub spec: ArchiveSpec,
    pub details: serde_json::Value,
    pub fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveRequest {
    pub version: u8,
    pub connection_generation: String,
    pub spec: ArchiveSpec,
    pub fingerprint: String,
}
impl ArchiveRequest {
    pub fn validate(&self) -> Result<u64, FileTransferError> {
        validate(&self.spec)?;
        if self.version != 1
            || self.fingerprint.len() != 64
            || !self.fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(failure("invalid", "invalid extraction confirmation"));
        }
        self.connection_generation
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| failure("invalid", "invalid extraction generation"))
    }
}
impl SshService {
    pub async fn preview_archive(
        &self,
        spec: ArchiveSpec,
        cancel: CancellationToken,
    ) -> Result<ArchivePreview, FileTransferError> {
        validate(&spec)?;
        let (owner, _) = crate::file_transfer::operation(
            &cancel,
            self.inner
                .file_client_for_generation(&spec.connection_id, None),
        )
        .await?;
        let generation = owner
            .transport_generation()
            .ok_or_else(|| failure("conflict", "connection unavailable"))?;
        let response = self
            .archive_helper(&spec, generation, "preview", None, &cancel)
            .await?;
        let details = response
            .get("preview")
            .cloned()
            .ok_or_else(|| failure("invalid", "archive preview missing"))?;
        let fingerprint = details["fingerprint"]
            .as_str()
            .ok_or_else(|| failure("invalid", "archive fingerprint missing"))?
            .to_owned();
        Ok(ArchivePreview {
            version: 1,
            connection_generation: generation.to_string(),
            spec,
            details,
            fingerprint,
        })
    }
    pub async fn execute_archive(
        &self,
        request: ArchiveRequest,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, FileTransferError> {
        let generation = request.validate()?;
        let result = self
            .archive_helper(
                &request.spec,
                generation,
                "extract",
                Some(&request.fingerprint),
                &cancel,
            )
            .await?;
        Ok(result["receipt"].clone())
    }
    async fn archive_helper(
        &self,
        spec: &ArchiveSpec,
        generation: u64,
        action: &str,
        fingerprint: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, FileTransferError> {
        let conflict =
            serde_json::to_value(spec.conflict).map_err(|e| failure("invalid", &e.to_string()))?;
        let payload = serde_json::json!({ "action": action, "archivePath": spec.archive_path, "destinationPath": spec.destination_path, "conflict": conflict, "fingerprint": fingerprint });
        let encoded = STANDARD
            .encode(serde_json::to_vec(&payload).map_err(|e| failure("invalid", &e.to_string()))?);
        let command = format!(
            "python3 -c {} {}",
            crate::remote_file_command::quote(include_str!("safe_archive.py")),
            crate::remote_file_command::quote(&encoded)
        );
        let output = crate::remote_file_command::run(
            self,
            &spec.connection_id,
            Some(generation),
            &command,
            cancel,
        )
        .await?;
        let value: serde_json::Value = serde_json::from_str(&output)
            .map_err(|e| failure("invalid", &format!("invalid archive helper result: {e}")))?;
        if value["ok"] != true {
            let mut error = failure(
                "archive",
                value["error"].as_str().unwrap_or("archive helper failed"),
            );
            if let Some(receipt) = value.get("receipt") {
                error
                    .cleanup_warnings
                    .push(format!("partial extraction: {receipt}"));
            }
            return Err(error);
        }
        Ok(value)
    }
}
fn validate(spec: &ArchiveSpec) -> Result<(), FileTransferError> {
    for path in [&spec.archive_path, &spec.destination_path] {
        crate::files::validate_absolute_path(path)
            .map_err(|e| failure("invalid", &e.to_string()))?;
    }
    if spec.connection_id.is_empty()
        || spec.connection_id.len() > 256
        || spec.destination_path == "/"
        || spec.conflict == crate::FileConflictPolicy::KeepBoth
    {
        return Err(failure(
            "invalid",
            "choose a destination below root and ask/skip/replace policy",
        ));
    }
    Ok(())
}
fn failure(code: &str, message: &str) -> FileTransferError {
    FileTransferError::new(code, message)
}
