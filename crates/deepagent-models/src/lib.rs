//! # deepagent-models
//!
//! DeepSeek-native model client (开发计划.md Phase 2 §4–§5).
//!
//! Implements the four Phase 2 model requirements:
//! - **SSE streaming** — [`sse::SseParser`] incrementally de-frames the byte
//!   stream with no data loss across split network chunks.
//! - **delta merge** — [`stream::DeltaAccumulator`] folds content /
//!   reasoning_content fragments into a coherent message.
//! - **tool_calls merge** — fragmented, index-addressed tool-call arguments are
//!   reassembled and JSON-parsed.
//! - **Thinking Mode** — `reasoning_content` is preserved through assembly so it
//!   can be persisted & replayed (see `deepagent_core::message`).
//!
//! ## Layering
//!
//! ```text
//! ChatRequest --serialize--> TransportRequest --HttpTransport--> SSE bytes
//!     SSE bytes --SseParser--> data: payloads --DeltaAccumulator--> ChatResponse
//! ```
//!
//! The transport is abstracted ([`transport::HttpTransport`]) so all assembly
//! logic is tested offline via [`transport::MockTransport`]. The real
//! `reqwest`-based transport is compiled only with `--features http`.

pub mod balance;
pub mod chat;
pub mod client;
pub mod discovery;
pub mod sse;
pub mod stream;
pub mod transport;
pub mod wire;

#[cfg(feature = "http")]
pub mod reqwest_transport;

pub use balance::{fetch_balance, BalanceInfo, BalanceResponse, BALANCE_PATH};
pub use chat::{
    ChatRequest, ChatResponse, FinishReason, FunctionSchema, StreamOptions, ThinkingConfig,
    ThinkingDepth, ThinkingToggle, ToolSchema, Usage,
};
pub use client::{ModelClient, ModelConfig};
pub use discovery::{ModelCatalog, ModelDiscovery, ModelInfo, ModelRole, DEEPSEEK_BASE_URL};
pub use stream::{DeltaAccumulator, DeltaObserver, NoopObserver};
pub use transport::{HttpTransport, MockTransport, TransportRequest};

#[cfg(feature = "http")]
pub use reqwest_transport::ReqwestTransport;
