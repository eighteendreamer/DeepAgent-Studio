//! The existing connection watch owns both closure and its first protocol cause.
use crate::SdkError;
use tokio::sync::watch;

/// Peer-controlled error strings/lines are intentionally never retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodeFailure {
    category: &'static str,
    line: usize,
    column: usize,
}

impl DecodeFailure {
    pub(crate) fn from_error(error: &serde_json::Error) -> Self {
        use serde_json::error::Category;
        Self {
            category: match error.classify() {
                Category::Io => "io",
                Category::Syntax => "syntax",
                Category::Data => "envelope",
                Category::Eof => "incomplete JSON",
            },
            line: error.line(),
            column: error.column(),
        }
    }

    fn error(self) -> SdkError {
        // Reconstruct the existing public Decode variant from safe diagnostics,
        // never from serde's potentially payload-bearing Display text.
        SdkError::Decode(<serde_json::Error as serde::de::Error>::custom(format!(
            "invalid stdout {} at line {} column {}; connection retired; reconcile durable state before retrying",
            self.category, self.line, self.column
        )))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ConnectionState {
    #[default]
    Open,
    Closed,
    DecodeFailed(DecodeFailure),
}

impl ConnectionState {
    pub(crate) fn is_closed(self) -> bool {
        self != Self::Open
    }

    pub(crate) fn decode_error(self) -> Option<SdkError> {
        match self {
            Self::DecodeFailed(error) => Some(error.error()),
            _ => None,
        }
    }

    /// First retirement wins. EOF, writer cleanup and explicit close cannot
    /// replace an already published decode cause (nor reopen the connection).
    pub(crate) fn retire(sender: &watch::Sender<Self>, reason: Self) {
        sender.send_if_modified(|state| {
            if state.is_closed() {
                return false;
            }
            *state = reason;
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h101_connection_retirement_is_single_owner_and_first_cause_wins() {
        let error =
            serde_json::from_str::<serde_json::Value>("not-json-PRIVATE_CANARY").unwrap_err();
        let bad = ConnectionState::DecodeFailed(DecodeFailure::from_error(&error));
        for first in [ConnectionState::Closed, bad] {
            let (state, _) = watch::channel(ConnectionState::Open);
            ConnectionState::retire(&state, first);
            ConnectionState::retire(&state, ConnectionState::Closed);
            ConnectionState::retire(&state, bad);
            assert_eq!(*state.borrow(), first);
            let error = state.borrow().decode_error();
            if let Some(error) = error {
                assert!(!error.to_string().contains("PRIVATE_CANARY"));
            }
        }
    }
}
