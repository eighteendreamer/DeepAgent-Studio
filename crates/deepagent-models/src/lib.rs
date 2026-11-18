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

#[cfg(test)]
extern crate self as deepagent_models;

#[cfg(test)]
#[path = "../tests/support/sequence.rs"]
pub mod test_sequence;

#[cfg(test)]
#[path = "../tests/support/sse_protocol_fixture.rs"]
pub(crate) mod test_wire;

pub mod balance;
pub mod capability;
pub mod chat;
pub mod chat_completions;
pub mod client;
pub mod discovery;
pub mod failure;
pub mod file_references;
pub mod image_input;
pub mod messages;
mod protocol;
mod provider_error;
pub mod responses;
pub mod sampling;
pub mod sse;
pub mod stream;
pub mod thinking;
pub mod tokenizer;
mod tool_choice;
pub mod transport;
pub mod usage;

#[cfg(feature = "http")]
pub mod reqwest_transport;

pub use balance::{
    fetch_balance, fetch_balance_by_kind, BalanceAmount, BalanceInfo, BalanceKind, BalanceResponse,
    BalanceSnapshot, BALANCE_PATH,
};
pub use capability::{CapabilitySource, ModelCapability, ModelCapabilityResolver};
pub use chat::{
    FinishReason, FunctionSchema, ReasoningEffort, Response, ResponseRequest, ThinkingToggle,
    ToolSchema, Usage,
};
pub use chat_completions::{ChatCompletionAccumulator, ChatCompletionRequest};
pub use client::{
    ModelClient, ModelConfig, ProviderToolSchemas, ResponseDefaults, WireMode,
    DEEPSEEK_OFFICIAL_PROVIDER,
};
pub use discovery::{ModelCatalog, ModelDiscovery, ModelInfo, ModelRole, DEEPSEEK_BASE_URL};
pub use failure::{classify_model_error, ModelFailureKind};
pub use responses::{
    messages_from_response_items, response_items_from_legacy_messages,
    response_items_from_messages, response_items_from_messages_with_native_calls,
    ResponseInputItem, ResponseItem, ResponseOutputItem,
};
pub use sampling::{resolve_responses_sampling, EffectiveSamplingConfig, SamplingReason};
pub use stream::{DeltaObserver, ModelStreamEvent, NoopObserver, ResponseAccumulator};
pub use tool_choice::validate_responses_tool_choice;
pub use transport::{
    FileUploadRequest, HttpTransport, MockTransport, TransportRequest, UploadedFile,
};

#[cfg(feature = "http")]
pub use reqwest_transport::{ReqwestTransport, TransportTimeouts};
