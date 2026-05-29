//! Strongly-typed identifiers.
//!
//! Using distinct newtypes (rather than passing raw `Uuid`/`String` around)
//! prevents accidentally using a `TaskId` where a `SessionId` is expected, a
//! class of bug that is otherwise easy to introduce in an agent runtime that
//! juggles many kinds of IDs.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Generates a newtype wrapper around [`Uuid`] with the common conveniences:
/// `new()` (v7, time-ordered), `nil()`, `Display`, `FromStr`, and serde.
macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(Uuid);

        impl $name {
            /// Create a new, time-ordered (UUID v7) identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// The nil identifier (all zeroes). Useful as a sentinel / default.
            pub const fn nil() -> Self {
                Self(Uuid::nil())
            }

            /// Wrap an existing [`Uuid`].
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            /// Borrow the underlying [`Uuid`].
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// The short prefix used when rendering this id type, e.g. `"ses"`.
            pub const PREFIX: &'static str = $prefix;
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}_{}", $prefix, self.0.simple())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self)
            }
        }

        impl std::str::FromStr for $name {
            type Err = crate::error::CoreError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                // Accept either the prefixed form ("ses_<hex>") or a bare UUID.
                let raw = s.strip_prefix(concat!($prefix, "_")).unwrap_or(s);
                Uuid::parse_str(raw)
                    .map(Self)
                    .map_err(|e| crate::error::CoreError::invalid(format!(
                        "invalid {}: {e}", stringify!($name)
                    )))
            }
        }
    };
}

typed_id!(
    /// Identifies a single agent session (one append-only event stream).
    SessionId, "ses"
);
typed_id!(
    /// Identifies a task within a session.
    TaskId, "task"
);
typed_id!(
    /// Identifies a single event in the append-only log.
    EventId, "evt"
);
typed_id!(
    /// Identifies an agent (root agent or sub-agent) instance.
    AgentId, "agt"
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn display_has_prefix() {
        let id = SessionId::new();
        assert!(id.to_string().starts_with("ses_"));
    }

    #[test]
    fn roundtrip_through_string() {
        let id = TaskId::new();
        let parsed = TaskId::from_str(&id.to_string()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn parses_bare_uuid() {
        let raw = Uuid::now_v7();
        let id = EventId::from_str(&raw.to_string()).unwrap();
        assert_eq!(id.as_uuid(), &raw);
    }

    #[test]
    fn ids_are_distinct_types() {
        // This is a compile-time guarantee; the test just documents intent.
        let s = SessionId::new();
        let t = TaskId::new();
        assert_ne!(s.to_string(), t.to_string());
    }

    #[test]
    fn v7_ids_are_time_ordered() {
        let a = EventId::new();
        let b = EventId::new();
        assert!(a < b, "v7 ids should be monotonically increasing");
    }
}
