//! # deepagent-models
//!
//! DeepSeek-native model client (开发计划.md Phase 2 §4–§5).
//!
//! Implements the four Phase 2 model requirements:
//! - **SSE streaming** — [`sse::SseParser`] incrementally de-frames the byte
//!   stream with no data loss across split network chunks.
//! - **semantic delta merge** — [`stream::ResponseAccumulator`] folds Responses
//!   text, reasoning, output items, and tool argument events coherently.
//! - **Reasoning** — Responses reasoning deltas are projected to the internal
//!   message model for persistence and UI display.
//!
//! ## Layering
//!
//! ```text
//! ResponseRequest --serialize--> TransportRequest --HttpTransport--> SSE bytes
//!     SSE bytes --SseParser--> semantic events --ResponseAccumulator--> Response
//! ```
//!
//! The transport is abstracted ([`transport::HttpTransport`]) so all assembly
//! logic is tested offline via [`transport::MockTransport`]. The real
//! `reqwest`-based transport is compiled only with `--features http`.

pub mod balance;
pub mod capability;
pub mod chat;
pub mod client;
pub mod discovery;
pub mod failure;
pub mod responses;
pub mod sse;
pub mod stream;
pub mod transport;

#[cfg(feature = "http")]
pub mod reqwest_transport;

pub use balance::{fetch_balance, BalanceInfo, BalanceResponse, BALANCE_PATH};
pub use capability::{CapabilitySource, ModelCapability, ModelCapabilityResolver};
pub use chat::{
    FinishReason, FunctionSchema, Response, ResponseRequest, StreamOptions, ThinkingConfig,
    ThinkingDepth, ThinkingToggle, ToolSchema, Usage,
};
pub use client::{ModelClient, ModelConfig, ResponseDefaults};
pub use discovery::{ModelCatalog, ModelDiscovery, ModelInfo, ModelRole, DEEPSEEK_BASE_URL};
pub use failure::{classify_model_error, ModelFailureKind};
pub use responses::{
    response_items_from_messages, ResponseInputItem, ResponseItem, ResponseOutputItem,
};
pub use stream::{DeltaObserver, ModelStreamEvent, NoopObserver, ResponseAccumulator};
pub use transport::{HttpTransport, MockTransport, TransportRequest};

#[cfg(feature = "http")]
pub use reqwest_transport::{ReqwestTransport, TransportTimeouts};
