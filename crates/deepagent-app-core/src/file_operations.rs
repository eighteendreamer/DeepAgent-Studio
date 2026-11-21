//! File workbench operations admitted to the existing durable transfer queue.
use deepagent_ssh::{ArchiveRequest, RemoteChmodRequest, ServerCopyRequest};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkbenchOperation {
    Chmod {
        request: RemoteChmodRequest,
    },
    Copy {
        request: ServerCopyRequest,
    },
    Extract {
        request: ArchiveRequest,
    },
    EditUpload {
        request: deepagent_ssh::ConditionalUploadRequest,
    },
}
impl WorkbenchOperation {
    pub fn connection_id(&self) -> &str {
        match self {
            Self::Chmod { request } => &request.connection_id,
            Self::Copy { request } => &request.spec.source_connection_id,
            Self::Extract { request } => &request.spec.connection_id,
            Self::EditUpload { request } => &request.connection_id,
        }
    }
    pub fn connections(&self) -> Vec<String> {
        let mut ids = vec![self.connection_id().to_owned()];
        if let Self::Copy { request } = self {
            ids.push(request.spec.destination_connection_id.clone());
        }
        ids.sort();
        ids.dedup();
        ids
    }
    pub fn validate(&self) -> Result<(), deepagent_ssh::FileTransferError> {
        match self {
            Self::Chmod { request } => request.validate().map(|_| ()),
            Self::Copy { request } => request.validate().map(|_| ()),
            Self::Extract { request } => request.validate().map(|_| ()),
            Self::EditUpload { request } => request.validate(),
        }
    }
}
