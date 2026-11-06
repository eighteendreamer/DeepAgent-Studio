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
    /// The peer's success result does not satisfy the initialize contract.
    #[error("initialize result has an invalid {field}; connection was not authorized")]
    InvalidInitializeResult {
        /// Contract field name only, never arbitrary peer content.
        field: &'static str,
    },
    /// The peer announced a different machine protocol (not JSON-RPC version).
    #[error("incompatible initialize protocol version: expected {expected}, actual {actual}")]
    IncompatibleProtocolVersion {
        /// Locally supported version.
        expected: u32,
        /// Unsigned numeric version received from the peer.
        actual: u64,
    },
    /// A local request deadline must be positive and representable by the clock.
    #[error("request timeout must be positive and representable")]
    InvalidRequestTimeout,
    /// Local wait ended; this is not a receipt of server cancellation or failure.
    #[error("local RPC deadline exceeded for {method} (id {id}, outcome_unknown={outcome_unknown}); reconcile thread/read before retrying; no server cancellation was sent")]
    RequestTimeout {
        /// Host-generated request identifier, not the response payload.
        id: i64,
        /// Typed protocol method; request parameters are never included.
        method: String,
        /// True once writing may have begun; even partial writes can be accepted.
        outcome_unknown: bool,
    },
    /// A successful response cannot identify the operation's event scope.
    #[error("{method} result has an invalid or conflicting {field}")]
    InvalidStreamIdentity {
        /// The request whose result could not be associated.
        method: &'static str,
        /// The missing, invalid or conflicting identifier (never its received value).
        field: &'static str,
    },
    /// A mutating RPC success receipt violated the typed v1 result contract.
    /// This is not evidence that the remote mutation did not occur.
    #[error(transparent)]
    InvalidMethodResult(#[from] deepagent_harness_protocol::ControlResultError),
    /// The bounded live receiver lost events; durable reconciliation is required.
    #[error("event stream lagged by {skipped} records; reconcile thread/read before retrying")]
    EventLagged {
        /// Number of overwritten live records, not a durable run cursor.
        skipped: u64,
    },
    /// The connection closed before this owned stream reached its terminal.
    #[error("event stream closed before terminal; reconcile thread/read before retrying")]
    EventStreamClosed,
    /// The live connection skipped wire sequence values; not durable row counts.
    #[error("event sequence gap {previous} -> {received} ({missing} missing); reconcile thread/read using durable cursors, not this wire sequence")]
    EventSequenceGap {
        /// Previous wire high-water value.
        previous: u64,
        /// Received wire value after the gap.
        received: u64,
        /// Missing wire sequence count.
        missing: u64,
    },
    /// The connection emitted a duplicate/descending sequence; no local reordering.
    #[error("event sequence is not increasing: previous={previous}, received={received}; reconcile thread/read before ACK")]
    EventSequenceNonMonotonic {
        /// Previous wire high-water value.
        previous: u64,
        /// Received duplicate/descending value.
        received: u64,
    },
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
