use thiserror::Error;

/// Errors produced by the harness SDK.
#[derive(Debug, Error)]
pub enum SdkError {
    /// Failed to spawn the `deepagent-cli` server process.
    #[error("spawn server process: {0}")]
    Spawn(std::io::Error),
    /// Failed to read a line from the server's stdout stream.
    #[error("read server stdout: {0}")]
    Read(std::io::Error),
    /// Failed to write a request to the server's stdin stream.
    #[error("write server stdin: {0}")]
    Write(std::io::Error),
    /// A line on stdout could not be decoded as a request/notification envelope.
    #[error("decode server line: {0}")]
    Decode(serde_json::Error),
    /// A request envelope could not be serialized.
    #[error("serialize request: {0}")]
    Encode(serde_json::Error),
    /// The server exited or closed its stdout before the response arrived.
    #[error("server closed while awaiting response {id}")]
    ResponseChannelClosed {
        /// The request id whose response was lost.
        id: i64,
    },
    /// A request was sent before the server was ready (e.g. before `initialize`).
    #[error("request rejected before initialize")]
    NotInitialized,
    /// The server responded with a JSON-RPC error.
    #[error("rpc error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code (see app-server `ERR_*` constants).
        code: i32,
        /// Human-readable error message from the server.
        message: String,
    },
    /// The workspace path could not be used as the server's working directory.
    #[error("use workspace {path:?} as server cwd: {source}")]
    Workspace {
        /// The requested workspace path.
        path: std::path::PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl From<serde_json::Error> for SdkError {
    fn from(error: serde_json::Error) -> Self {
        SdkError::Encode(error)
    }
}
