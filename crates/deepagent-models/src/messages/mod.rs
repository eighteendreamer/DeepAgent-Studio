//! Anthropic Messages projection behind the existing model/transport boundary.
//!
//! SDK types define the wire schema. Host-owned streaming state and usage
//! conversion retain the neutral result and physical-call accounting contracts.

mod request;
mod stream;
mod usage;

pub(crate) use request::{encode_request_with_cache, project_tools, CachePolicy};
pub use stream::MessagesAccumulator;

/// Messages API roots may include a provider prefix and an explicit `/v1`.
/// The user configures the root; inference and discovery append resources.
pub(crate) fn api_root(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        base.to_owned()
    } else {
        format!("{base}/v1")
    }
}
