//! Stable classification of provider and transport failures.

use deepagent_core::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFailureKind {
    Cancelled,
    Authentication,
    Permission,
    RateLimited,
    Overloaded,
    Server,
    ContextOverflow,
    RequestTooLarge,
    Timeout,
    Transport,
    InvalidRequest,
    Unknown,
}

impl ModelFailureKind {
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Overloaded | Self::Server | Self::Timeout | Self::Transport
        )
    }

    pub const fn should_fallback(self) -> bool {
        matches!(self, Self::Overloaded)
    }
}

pub fn classify_model_error(error: &CoreError) -> ModelFailureKind {
    match error {
        CoreError::Provider {
            status,
            code,
            message,
        } => classify_provider(*status, code.as_deref(), message),
        CoreError::Serialization(_) | CoreError::Invalid(_) => ModelFailureKind::InvalidRequest,
        other => classify_unstructured(&other.to_string()),
    }
}

fn classify_provider(status: Option<u16>, code: Option<&str>, message: &str) -> ModelFailureKind {
    let detail = format!("{} {}", code.unwrap_or_default(), message).to_ascii_lowercase();
    if is_context_overflow(&detail) || status == Some(413) && detail.contains("token") {
        return ModelFailureKind::ContextOverflow;
    }
    if status == Some(413) {
        return ModelFailureKind::RequestTooLarge;
    }
    if status == Some(401) {
        return ModelFailureKind::Authentication;
    }
    if status == Some(403) {
        return ModelFailureKind::Permission;
    }
    if status == Some(429) {
        return if detail.contains("overload") || detail.contains("high demand") {
            ModelFailureKind::Overloaded
        } else {
            ModelFailureKind::RateLimited
        };
    }
    if status == Some(529) || detail.contains("overloaded_error") {
        return ModelFailureKind::Overloaded;
    }
    if status.is_some_and(|status| (500..=599).contains(&status)) {
        return ModelFailureKind::Server;
    }
    if status.is_some_and(|status| (400..=499).contains(&status)) {
        return ModelFailureKind::InvalidRequest;
    }
    classify_unstructured(&detail)
}

fn classify_unstructured(message: &str) -> ModelFailureKind {
    let message = message.to_ascii_lowercase();
    if message.contains("cancelled") || message.contains("canceled") {
        ModelFailureKind::Cancelled
    } else if message.contains("overloaded") || message.contains("high demand") {
        ModelFailureKind::Overloaded
    } else if message.contains("http 401") || message.contains("returned 401") {
        ModelFailureKind::Authentication
    } else if message.contains("http 403") || message.contains("returned 403") {
        ModelFailureKind::Permission
    } else if message.contains("http 429")
        || message.contains("returned 429")
        || message.contains("too many requests")
    {
        ModelFailureKind::RateLimited
    } else if [
        "http 500",
        "http 502",
        "http 503",
        "http 504",
        "returned 500",
        "returned 502",
        "returned 503",
        "returned 504",
    ]
    .iter()
    .any(|status| message.contains(status))
    {
        ModelFailureKind::Server
    } else if is_context_overflow(&message) {
        ModelFailureKind::ContextOverflow
    } else if message.contains("timed out")
        || message.contains("timeout")
        || message.contains("deadline exceeded")
    {
        ModelFailureKind::Timeout
    } else if message.contains("connection")
        || message.contains("unexpected eof")
        || message.contains("incomplete message")
        || message.contains("empty stream")
        || message.contains("empty model stream")
        || message.contains("without a terminal response event")
    {
        ModelFailureKind::Transport
    } else {
        ModelFailureKind::Unknown
    }
}

fn is_context_overflow(message: &str) -> bool {
    [
        "context_length_exceeded",
        "context length",
        "context window",
        "prompt is too long",
        "prompt too long",
        "maximum context",
        "too many tokens",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_structured_provider_failures() {
        assert_eq!(
            classify_model_error(&CoreError::provider(
                Some(413),
                Some("context_length_exceeded".into()),
                "maximum context window exceeded",
            )),
            ModelFailureKind::ContextOverflow
        );
        assert_eq!(
            classify_model_error(&CoreError::provider(
                Some(529),
                Some("overloaded_error".into()),
                "high demand",
            )),
            ModelFailureKind::Overloaded
        );
        assert_eq!(
            classify_model_error(&CoreError::provider(Some(401), None, "invalid key")),
            ModelFailureKind::Authentication
        );
    }

    #[test]
    fn only_transient_failures_retry() {
        assert!(ModelFailureKind::RateLimited.retryable());
        assert!(ModelFailureKind::Transport.retryable());
        assert!(!ModelFailureKind::ContextOverflow.retryable());
        assert!(!ModelFailureKind::Authentication.retryable());
        assert_eq!(
            classify_model_error(&CoreError::other("model API returned HTTP 429")),
            ModelFailureKind::RateLimited
        );
        assert_eq!(
            classify_model_error(&CoreError::other("model API returned HTTP 503")),
            ModelFailureKind::Server
        );
    }
}
