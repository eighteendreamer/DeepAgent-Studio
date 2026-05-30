//! A minimal YAML-frontmatter splitter for `SKILL.md`.
//!
//! `SKILL.md` files open with a `---` delimited block of simple `key: value`
//! pairs followed by the Markdown body. We only need the handful of scalar
//! keys Claude Code uses (`name`, `description`, `version`, `license`), so we
//! hand-roll a tiny line parser rather than pulling a full YAML crate — keeping
//! the crate dependency-light and offline-buildable.
//!
//! Supported syntax:
//! - `key: value` (value trimmed; surrounding single/double quotes stripped)
//! - blank lines and `# comments` inside the block are ignored
//! - everything after the closing `---` is the body (verbatim)

use std::collections::BTreeMap;

/// A parsed frontmatter document: scalar key/value pairs plus the body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frontmatter {
    /// Scalar key/value pairs from the `---` block.
    pub fields: BTreeMap<String, String>,
    /// The Markdown body following the frontmatter block.
    pub body: String,
}

impl Frontmatter {
    /// Look up a field by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(|s| s.as_str())
    }
}

/// Strip matching surrounding single or double quotes from `s`.
fn unquote(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Parse a `SKILL.md`-style document into [`Frontmatter`].
///
/// If the input does not start with a `---` line, the whole input is treated as
/// the body and `fields` is empty. A frontmatter block that is opened but never
/// closed is tolerated: parsing stops at end-of-input and the remaining lines
/// are treated as fields.
pub fn parse(input: &str) -> Frontmatter {
    // Normalize newlines so CRLF files parse identically.
    let normalized = input.replace("\r\n", "\n");
    let mut lines = normalized.lines();

    // Frontmatter must start with a `---` fence on the first line.
    let first = lines.next();
    if first.map(str::trim) != Some("---") {
        return Frontmatter {
            fields: BTreeMap::new(),
            body: normalized.trim_start_matches('\n').to_string(),
        };
    }

    let mut fields = BTreeMap::new();
    let mut closed = false;
    let mut consumed_lines = 1; // the opening fence

    for line in lines.clone() {
        consumed_lines += 1;
        if line.trim() == "---" {
            closed = true;
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim().to_string();
            let value = unquote(value.trim()).to_string();
            if !key.is_empty() {
                fields.insert(key, value);
            }
        }
    }

    // The body is everything after the consumed frontmatter lines.
    let body = if closed {
        normalized
            .split('\n')
            .skip(consumed_lines)
            .collect::<Vec<_>>()
            .join("\n")
            .trim_start_matches('\n')
            .to_string()
    } else {
        String::new()
    };

    Frontmatter { fields, body }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_frontmatter() {
        let input =
            "---\nname: My Skill\ndescription: Does things\nversion: 0.1.0\n---\n# Body\n\nHello.";
        let fm = parse(input);
        assert_eq!(fm.get("name"), Some("My Skill"));
        assert_eq!(fm.get("description"), Some("Does things"));
        assert_eq!(fm.get("version"), Some("0.1.0"));
        assert!(fm.body.starts_with("# Body"));
        assert!(fm.body.contains("Hello."));
    }

    #[test]
    fn strips_quotes() {
        let input = "---\nname: \"Quoted Name\"\ndesc: 'single'\n---\nbody";
        let fm = parse(input);
        assert_eq!(fm.get("name"), Some("Quoted Name"));
        assert_eq!(fm.get("desc"), Some("single"));
    }

    #[test]
    fn handles_colons_in_value() {
        let input = "---\ndescription: Use when user says \"do: this\"\n---\nbody";
        let fm = parse(input);
        assert_eq!(
            fm.get("description"),
            Some("Use when user says \"do: this\"")
        );
    }

    #[test]
    fn no_frontmatter_is_all_body() {
        let input = "# Just a doc\n\nNo frontmatter here.";
        let fm = parse(input);
        assert!(fm.fields.is_empty());
        assert_eq!(fm.body, input);
    }

    #[test]
    fn crlf_is_normalized() {
        let input = "---\r\nname: X\r\n---\r\nbody line";
        let fm = parse(input);
        assert_eq!(fm.get("name"), Some("X"));
        assert_eq!(fm.body, "body line");
    }

    #[test]
    fn ignores_comments_and_blanks() {
        let input = "---\n# a comment\n\nname: X\n---\nbody";
        let fm = parse(input);
        assert_eq!(fm.get("name"), Some("X"));
        assert_eq!(fm.fields.len(), 1);
    }
}
