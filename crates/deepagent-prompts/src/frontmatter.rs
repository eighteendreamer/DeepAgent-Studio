//! A minimal YAML-frontmatter splitter for command/agent `.md` files.
//!
//! Supports the scalar keys Claude Code uses in command/agent frontmatter
//! (`name`, `description`, `allowed-tools`, `tools`, `model`, `color`,
//! `argument-hint`, `disable-model-invocation`). List-valued keys are accepted
//! either as inline comma-separated values (`Bash(git:*), Read`) or as a YAML
//! block list (`- item` lines). We hand-roll this to stay dependency-light and
//! offline-buildable rather than pulling a full YAML crate.

use std::collections::BTreeMap;

/// A parsed frontmatter document: scalar fields + list fields + body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Frontmatter {
    /// Scalar `key: value` pairs.
    pub fields: BTreeMap<String, String>,
    /// List-valued keys (inline comma list or YAML block list).
    pub lists: BTreeMap<String, Vec<String>>,
    /// The Markdown body following the frontmatter block.
    pub body: String,
}

impl Frontmatter {
    /// Look up a scalar field.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(|s| s.as_str())
    }

    /// Look up a scalar field as a bool (`true`/`yes`/`1`).
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.fields.get(key).map(|v| {
            let v = v.trim().to_lowercase();
            v == "true" || v == "yes" || v == "1"
        })
    }

    /// Resolve a list-valued key, accepting either an explicit block list or a
    /// comma-separated scalar value.
    pub fn get_list(&self, key: &str) -> Vec<String> {
        if let Some(list) = self.lists.get(key) {
            return list.clone();
        }
        if let Some(scalar) = self.fields.get(key) {
            return split_csv(scalar);
        }
        Vec::new()
    }
}

/// Split a comma-separated value into trimmed, non-empty items.
fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Strip matching surrounding single/double quotes.
fn unquote(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 {
        let (f, l) = (b[0], b[b.len() - 1]);
        if (f == b'"' && l == b'"') || (f == b'\'' && l == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Parse a command/agent `.md` document into [`Frontmatter`].
///
/// If the input does not open with `---`, the whole input is the body.
pub fn parse(input: &str) -> Frontmatter {
    let normalized = input.replace("\r\n", "\n");
    let mut lines = normalized.lines();

    if lines.next().map(str::trim) != Some("---") {
        return Frontmatter {
            body: normalized.trim_start_matches('\n').to_string(),
            ..Default::default()
        };
    }

    let mut fields = BTreeMap::new();
    let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut closed = false;
    let mut consumed = 1usize;
    let mut current_list_key: Option<String> = None;

    for line in lines.clone() {
        consumed += 1;
        if line.trim() == "---" {
            closed = true;
            break;
        }
        let raw = line;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // YAML block-list item under the most recent `key:` with empty value.
        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some(key) = &current_list_key {
                lists
                    .entry(key.clone())
                    .or_default()
                    .push(unquote(item.trim()).to_string());
                continue;
            }
        }

        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim();
            if key.is_empty() {
                continue;
            }
            if value.is_empty() {
                // Possibly the header of a block list; remember the key.
                current_list_key = Some(key.clone());
                lists.entry(key).or_default();
            } else {
                current_list_key = None;
                fields.insert(key, unquote(value).to_string());
            }
        }
    }

    // Drop empty list entries that never received items (they were scalars
    // mistaken as list headers but had no following items).
    lists.retain(|_, v| !v.is_empty());

    let body = if closed {
        normalized
            .split('\n')
            .skip(consumed)
            .collect::<Vec<_>>()
            .join("\n")
            .trim_start_matches('\n')
            .to_string()
    } else {
        String::new()
    };

    Frontmatter {
        fields,
        lists,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_and_body() {
        let fm = parse(
            "---\nname: code-architect\nmodel: sonnet\ncolor: green\n---\nYou are an architect.",
        );
        assert_eq!(fm.get("name"), Some("code-architect"));
        assert_eq!(fm.get("model"), Some("sonnet"));
        assert!(fm.body.starts_with("You are an architect"));
    }

    #[test]
    fn inline_csv_list() {
        let fm = parse("---\ntools: Glob, Grep, Read, TodoWrite\n---\nbody");
        assert_eq!(
            fm.get_list("tools"),
            vec!["Glob", "Grep", "Read", "TodoWrite"]
        );
    }

    #[test]
    fn yaml_block_list() {
        let fm = parse("---\nallowed-tools:\n  - Bash(git:*)\n  - Read\n---\nbody");
        assert_eq!(fm.get_list("allowed-tools"), vec!["Bash(git:*)", "Read"]);
    }

    #[test]
    fn bool_field() {
        let fm = parse("---\ndisable-model-invocation: true\n---\nb");
        assert_eq!(fm.get_bool("disable-model-invocation"), Some(true));
    }

    #[test]
    fn no_frontmatter_all_body() {
        let fm = parse("# Just body\ncontent");
        assert!(fm.fields.is_empty());
        assert_eq!(fm.body, "# Just body\ncontent");
    }

    #[test]
    fn colon_in_value_preserved() {
        let fm = parse("---\ndescription: Use when: do a thing\n---\nb");
        assert_eq!(fm.get("description"), Some("Use when: do a thing"));
    }
}
