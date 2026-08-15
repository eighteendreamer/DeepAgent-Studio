//! Plugin name validation per Agent Plugins Specification 1.0.0 §5.5.
//!
//! The constraints are:
//!
//! | Constraint    | Requirement                                          |
//! | ------------- | ---------------------------------------------------- |
//! | Length        | 1–64 characters inclusive                            |
//! | Character set | `a-z`, `0-9`, `-`, `.` only                          |
//! | Start and end | first and last characters must be alphanumeric       |
//! | Repetition    | no `--` and no `..`                                  |
//!
//! [`PluginName`] validates on construction, so a value of this type is always
//! spec-conformant. §5.3 makes a missing, wrongly typed, or empty `name` fatal
//! to the plugin, which is why [`PluginName::parse`] returns an error rather
//! than sanitizing its input.

use std::fmt;

/// Inclusive upper bound on name length (§5.5).
pub const MAX_PLUGIN_NAME_LEN: usize = 64;

/// A plugin name that satisfies §5.5.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginName(String);

/// Which §5.5 constraint a candidate name violated. Reported so the user can
/// see *why* a plugin was rejected (§5.3 recommends naming the invalid field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginNameError {
    /// Empty after no trimming is applied — the spec has no trimming rule.
    Empty,
    /// Longer than [`MAX_PLUGIN_NAME_LEN`] characters.
    TooLong { len: usize },
    /// A character outside `a-z`, `0-9`, `-`, `.`.
    InvalidChar { ch: char, index: usize },
    /// First or last character is not alphanumeric.
    BoundaryNotAlphanumeric,
    /// Contains `--`.
    ConsecutiveHyphen,
    /// Contains `..`.
    ConsecutivePeriod,
}

impl fmt::Display for PluginNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "plugin name must not be empty"),
            Self::TooLong { len } => write!(
                f,
                "plugin name must be at most {MAX_PLUGIN_NAME_LEN} characters, got {len}"
            ),
            Self::InvalidChar { ch, index } => write!(
                f,
                "plugin name may only contain lowercase letters, digits, '-' and '.'; \
                 found {ch:?} at index {index}"
            ),
            Self::BoundaryNotAlphanumeric => write!(
                f,
                "plugin name must start and end with a lowercase letter or digit"
            ),
            Self::ConsecutiveHyphen => {
                write!(f, "plugin name must not contain consecutive hyphens")
            }
            Self::ConsecutivePeriod => {
                write!(f, "plugin name must not contain consecutive periods")
            }
        }
    }
}

impl std::error::Error for PluginNameError {}

impl PluginName {
    /// Validates `raw` against §5.5.
    ///
    /// The input is used verbatim: the spec defines no normalization, so
    /// accepting `" demo "` here would let a plugin claim a name no other
    /// client agrees with.
    pub fn parse(raw: &str) -> Result<Self, PluginNameError> {
        if raw.is_empty() {
            return Err(PluginNameError::Empty);
        }

        // Count characters, not bytes: §5.5 states a character bound, and a
        // non-ASCII input must fail on the character set rule below rather than
        // on a byte-length technicality.
        let len = raw.chars().count();
        if len > MAX_PLUGIN_NAME_LEN {
            return Err(PluginNameError::TooLong { len });
        }

        for (index, ch) in raw.char_indices() {
            if !is_allowed(ch) {
                return Err(PluginNameError::InvalidChar { ch, index });
            }
        }

        // Safe to index: `raw` is non-empty and every character is ASCII by the
        // check above.
        let bytes = raw.as_bytes();
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if !is_alphanumeric_byte(first) || !is_alphanumeric_byte(last) {
            return Err(PluginNameError::BoundaryNotAlphanumeric);
        }

        if raw.contains("--") {
            return Err(PluginNameError::ConsecutiveHyphen);
        }
        if raw.contains("..") {
            return Err(PluginNameError::ConsecutivePeriod);
        }

        Ok(Self(raw.to_string()))
    }

    /// The validated name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper, yielding the validated name.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for PluginName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for PluginName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// §5.5 character set: lowercase alphanumerics, hyphen, period.
fn is_allowed(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '.'
}

fn is_alphanumeric_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact valid examples listed in §5.5.
    #[test]
    fn accepts_spec_valid_examples() {
        for name in ["my-plugin", "acme.tools", "lint3r", "a"] {
            let parsed = PluginName::parse(name)
                .unwrap_or_else(|e| panic!("expected {name:?} to be valid, got {e}"));
            assert_eq!(parsed.as_str(), name);
        }
    }

    /// The exact invalid examples listed in §5.5, each mapped to its cause.
    #[test]
    fn rejects_spec_invalid_examples() {
        assert_eq!(
            PluginName::parse("My-Plugin"),
            Err(PluginNameError::InvalidChar { ch: 'M', index: 0 })
        );
        assert_eq!(
            PluginName::parse("-start"),
            Err(PluginNameError::BoundaryNotAlphanumeric)
        );
        assert_eq!(
            PluginName::parse("has--double"),
            Err(PluginNameError::ConsecutiveHyphen)
        );
        assert_eq!(
            PluginName::parse("too.many..dots"),
            Err(PluginNameError::ConsecutivePeriod)
        );
        assert_eq!(PluginName::parse(""), Err(PluginNameError::Empty));
    }

    #[test]
    fn length_boundaries() {
        let max = "a".repeat(MAX_PLUGIN_NAME_LEN);
        assert!(PluginName::parse(&max).is_ok());

        let over = "a".repeat(MAX_PLUGIN_NAME_LEN + 1);
        assert_eq!(
            PluginName::parse(&over),
            Err(PluginNameError::TooLong {
                len: MAX_PLUGIN_NAME_LEN + 1
            })
        );
    }

    #[test]
    fn trailing_and_leading_period_rejected() {
        assert_eq!(
            PluginName::parse(".start"),
            Err(PluginNameError::BoundaryNotAlphanumeric)
        );
        assert_eq!(
            PluginName::parse("end."),
            Err(PluginNameError::BoundaryNotAlphanumeric)
        );
        assert_eq!(
            PluginName::parse("end-"),
            Err(PluginNameError::BoundaryNotAlphanumeric)
        );
    }

    #[test]
    fn mixed_separators_are_allowed_when_not_repeated() {
        assert!(PluginName::parse("a.b-c").is_ok());
        assert!(PluginName::parse("a-b.c-d").is_ok());
        assert!(PluginName::parse("a1.2b-3").is_ok());
    }

    #[test]
    fn rejects_uppercase_underscore_slash_and_whitespace() {
        assert!(matches!(
            PluginName::parse("Demo"),
            Err(PluginNameError::InvalidChar { .. })
        ));
        assert!(matches!(
            PluginName::parse("a_b"),
            Err(PluginNameError::InvalidChar { .. })
        ));
        assert!(matches!(
            PluginName::parse("a/b"),
            Err(PluginNameError::InvalidChar { .. })
        ));
        assert!(matches!(
            PluginName::parse("a b"),
            Err(PluginNameError::InvalidChar { .. })
        ));
    }

    /// A name is not trimmed: the spec defines no normalization, and silently
    /// trimming would make us disagree with other clients about the identity.
    #[test]
    fn does_not_trim_surrounding_whitespace() {
        assert!(matches!(
            PluginName::parse(" demo "),
            Err(PluginNameError::InvalidChar { ch: ' ', index: 0 })
        ));
    }

    /// Path traversal shapes are rejected by the character set, boundary, and
    /// repetition rules, which is what keeps a name safe to use in a path
    /// segment. Which rule fires first is an ordering detail of
    /// [`PluginName::parse`]; what matters is that none of these parse.
    #[test]
    fn rejects_path_traversal_shapes() {
        // The separator is caught by the character set before the repetition
        // rule is ever reached.
        assert!(matches!(
            PluginName::parse("../evil"),
            Err(PluginNameError::InvalidChar { ch: '/', .. })
        ));
        assert!(matches!(
            PluginName::parse("a/../b"),
            Err(PluginNameError::InvalidChar { ch: '/', .. })
        ));
        assert!(matches!(
            PluginName::parse("a\\b"),
            Err(PluginNameError::InvalidChar { ch: '\\', .. })
        ));
        // No separator: the leading dot fails the boundary rule.
        assert_eq!(
            PluginName::parse("..evil"),
            Err(PluginNameError::BoundaryNotAlphanumeric)
        );
        // Interior traversal shape with legal boundaries fails the repetition
        // rule.
        assert_eq!(
            PluginName::parse("a..b"),
            Err(PluginNameError::ConsecutivePeriod)
        );
    }

    #[test]
    fn non_ascii_rejected_by_character_set() {
        assert!(matches!(
            PluginName::parse("插件"),
            Err(PluginNameError::InvalidChar { .. })
        ));
    }

    #[test]
    fn error_messages_name_the_violated_constraint() {
        assert!(PluginNameError::ConsecutiveHyphen
            .to_string()
            .contains("consecutive hyphens"));
        assert!(PluginNameError::BoundaryNotAlphanumeric
            .to_string()
            .contains("start and end"));
        assert!(PluginNameError::TooLong { len: 65 }
            .to_string()
            .contains("64"));
    }
}
