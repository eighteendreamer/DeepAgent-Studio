//! Anthropic Messages projection behind the existing model/transport boundary.
//!
//! SDK types define the wire schema. Host-owned streaming state and usage
//! conversion retain the neutral result and physical-call accounting contracts.

mod request;
mod stream;
mod usage;

pub(crate) use request::{encode_request, project_tools};
pub use stream::MessagesAccumulator;
