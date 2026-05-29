//! Unified error type for the runtime kernel.
//!
//! Lower layers (persistence, network) wrap their concrete errors into
//! [`CoreError`] so that runtime logic can match on a single, stable enum.

use std::fmt;

/// Convenience alias used throughout the workspace.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;

/// The unified error type for the DeepAgent runtime.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// A value failed validation before it could be used.
    #[error("invalid input: {0}")]
    Invalid(String),

    /// A requested entity (session, task, event) was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// An operation was attempted in a state that does not permit it,
    /// e.g. an illegal task state transition.
    #[error("illegal state transition: {from} -> {to}")]
    IllegalTransition {
        /// The state the entity was in.
        from: String,
        /// The state that was illegally requested.
        to: String,
    },

    /// Serialization / deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// A persistence-layer failure (database, IO).
    #[error("persistence error: {0}")]
    Persistence(String),

    /// The event log is corrupt or inconsistent (e.g. sequence gap).
    #[error("event log corruption: {0}")]
    EventLog(String),

    /// A catch-all for errors that do not yet have a dedicated variant.
    #[error("{0}")]
    Other(String),
}

impl CoreError {
    /// Helper for building an [`CoreError::Invalid`] from anything stringy.
    pub fn invalid(msg: impl fmt::Display) -> Self {
        CoreError::Invalid(msg.to_string())
    }

    /// Helper for building a [`CoreError::NotFound`].
    pub fn not_found(msg: impl fmt::Display) -> Self {
        CoreError::NotFound(msg.to_string())
    }

    /// Helper for building a [`CoreError::Other`].
    pub fn other(msg: impl fmt::Display) -> Self {
        CoreError::Other(msg.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(value: serde_json::Error) -> Self {
        CoreError::Serialization(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_are_stable() {
        let e = CoreError::IllegalTransition {
            from: "Completed".into(),
            to: "Running".into(),
        };
        assert_eq!(
            e.to_string(),
            "illegal state transition: Completed -> Running"
        );
    }

    #[test]
    fn serde_error_converts() {
        let err: Result<i32> = serde_json::from_str::<i32>("not json").map_err(Into::into);
        assert!(matches!(err, Err(CoreError::Serialization(_))));
    }
}
