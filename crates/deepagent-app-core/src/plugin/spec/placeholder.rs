//! Plugin variable expansion per Agent Plugins Specification 1.0.0 §9.2.
//!
//! The spec is narrow and deliberately so:
//!
//! - Only `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` are expanded.
//! - Expansion is a single, non-recursive textual replacement of every exact
//!   occurrence. Text introduced by a replacement MUST NOT be scanned again.
//! - It applies to every string element of `args`, every string *value* in
//!   `env`, and `cwd`. It does not apply to `env` keys, `command`, or fixed
//!   component locations.
//! - Unrecognized placeholder-like text MUST remain literal, and clients MUST
//!   NOT perform any other placeholder or environment-variable expansion.
//!
//! # Why this is separate from `deepagent_mcp::config::expand`
//!
//! The shared MCP expander resolves arbitrary environment variables and turns
//! unknown ones into empty strings (its `unknown_var_expands_empty` test pins
//! that). User-authored MCP servers legitimately depend on that behavior — a
//! `${API_TOKEN}` in a hand-written config must still resolve. Changing it
//! would be a cross-cutting regression for configuration this feature has no
//! business touching.
//!
//! So plugin-sourced configuration gets its own expander with v1 semantics, and
//! the shared one keeps serving user- and project-sourced configuration. A
//! plugin-sourced value must pass through [`expand_v1`] and **not** through the
//! shared expander, or a literal `${FOO}` the spec requires us to preserve
//! would be silently eaten by a second pass.

use std::collections::BTreeMap;

/// The plugin root placeholder (§9.1).
pub const PLUGIN_ROOT_VAR: &str = "PLUGIN_ROOT";

/// The plugin data directory placeholder (§9.1).
pub const PLUGIN_DATA_VAR: &str = "PLUGIN_DATA";

/// Variable names an MCP server's `env` object must not declare, because the
/// client supplies them itself (§9.2). Declaring either invalidates the server
/// configuration under §7.2.2.
pub const RESERVED_ENV_VARS: &[&str] = &[PLUGIN_ROOT_VAR, PLUGIN_DATA_VAR];

/// Dialect aliases rewritten to the portable names before expansion.
///
/// Claude Code plugins use `${CLAUDE_PLUGIN_ROOT}` (the bundled Anthropic
/// plugins do this in their `hooks.json` commands). Normalizing here keeps
/// [`expand_v1`] itself limited to the two portable names, so dialect quirks
/// never leak into the spec-conformant core.
const DIALECT_ALIASES: &[(&str, &str)] = &[
    ("CLAUDE_PLUGIN_ROOT", PLUGIN_ROOT_VAR),
    ("CLAUDE_PLUGIN_DATA", PLUGIN_DATA_VAR),
    ("CODEX_PLUGIN_ROOT", PLUGIN_ROOT_VAR),
    ("CODEX_PLUGIN_DATA", PLUGIN_DATA_VAR),
    ("DEEPAGENT_PLUGIN_ROOT", PLUGIN_ROOT_VAR),
    ("DEEPAGENT_PLUGIN_DATA", PLUGIN_DATA_VAR),
];

/// Expands `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` with v1 semantics.
///
/// Single pass, non-recursive: replacement text is copied out verbatim and
/// never rescanned. Any other `${...}` sequence — and any unterminated `${` —
/// is preserved byte for byte.
pub fn expand_v1(input: &str, root: &str, data: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];

        let Some(end) = after_open.find('}') else {
            // Unterminated: the remainder is literal text, including the `${`.
            out.push_str(&rest[start..]);
            return out;
        };

        let var = &after_open[..end];
        match var {
            PLUGIN_ROOT_VAR => out.push_str(root),
            PLUGIN_DATA_VAR => out.push_str(data),
            // §9.2: unrecognized placeholder-like text remains literal.
            _ => out.push_str(&rest[start..start + 2 + end + 1]),
        }

        rest = &after_open[end + 1..];
    }

    out.push_str(rest);
    out
}

/// Rewrites client-specific placeholder aliases to the portable names.
///
/// Run this before [`expand_v1`] on plugin-sourced values. Unknown variables
/// are untouched, so this is safe to apply to any string.
pub fn rewrite_dialect_aliases(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];

        let Some(end) = after_open.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };

        let var = &after_open[..end];
        match DIALECT_ALIASES.iter().find(|(alias, _)| *alias == var) {
            Some((_, portable)) => {
                out.push_str("${");
                out.push_str(portable);
                out.push('}');
            }
            None => out.push_str(&rest[start..start + 2 + end + 1]),
        }

        rest = &after_open[end + 1..];
    }

    out.push_str(rest);
    out
}

/// Normalizes then expands a plugin-sourced value in one step.
pub fn normalize_and_expand(input: &str, root: &str, data: &str) -> String {
    expand_v1(&rewrite_dialect_aliases(input), root, data)
}

/// Returns the first reserved variable name declared as an `env` key, if any.
///
/// §9.2 forbids an MCP server's `env` from carrying `PLUGIN_ROOT` or
/// `PLUGIN_DATA`; such an entry makes the server configuration invalid.
pub fn reserved_env_key<V>(env: &BTreeMap<String, V>) -> Option<&'static str> {
    RESERVED_ENV_VARS
        .iter()
        .copied()
        .find(|reserved| env.contains_key(*reserved))
}

/// The three shapes §7.2.1 permits for an explicit MCP server `cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CwdForm<'a> {
    /// A plugin-relative path beginning with `./`.
    PluginRelative(&'a str),
    /// Exactly `${PLUGIN_ROOT}`, or a path beginning with `${PLUGIN_ROOT}/`.
    /// The tail excludes the leading separator and is empty for the bare form.
    PluginRoot { tail: &'a str },
    /// Exactly `${PLUGIN_DATA}`, or a path beginning with `${PLUGIN_DATA}/`.
    PluginData { tail: &'a str },
}

/// Classifies an explicit `cwd` value against the three permitted forms.
///
/// Syntax only: the caller still expands the value and enforces containment
/// against the corresponding root, since §7.2.1 makes a post-resolution escape
/// invalid too.
pub fn classify_cwd(raw: &str) -> Option<CwdForm<'_>> {
    if let Some(tail) = match_var_rooted(raw, PLUGIN_ROOT_VAR) {
        return Some(CwdForm::PluginRoot { tail });
    }
    if let Some(tail) = match_var_rooted(raw, PLUGIN_DATA_VAR) {
        return Some(CwdForm::PluginData { tail });
    }
    if raw.starts_with("./") {
        return Some(CwdForm::PluginRelative(raw));
    }
    None
}

/// Matches `${VAR}` exactly, or `${VAR}/tail`, returning the tail.
fn match_var_rooted<'a>(raw: &'a str, var: &str) -> Option<&'a str> {
    let prefix = format!("${{{var}}}");
    let rest = raw.strip_prefix(&prefix)?;
    if rest.is_empty() {
        return Some("");
    }
    // Only a separator may follow; `${PLUGIN_ROOT}x` is not one of the three
    // permitted forms.
    rest.strip_prefix('/').or_else(|| rest.strip_prefix('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/plugins/demo";
    const DATA: &str = "/data/demo";

    #[test]
    fn expands_both_portable_variables() {
        assert_eq!(
            expand_v1("${PLUGIN_ROOT}/bin/server", ROOT, DATA),
            "/plugins/demo/bin/server"
        );
        assert_eq!(
            expand_v1("${PLUGIN_DATA}/cache", ROOT, DATA),
            "/data/demo/cache"
        );
    }

    /// §9.2 says "every exact occurrence".
    #[test]
    fn expands_every_occurrence() {
        assert_eq!(
            expand_v1("${PLUGIN_ROOT}:${PLUGIN_DATA}:${PLUGIN_ROOT}", ROOT, DATA),
            "/plugins/demo:/data/demo:/plugins/demo"
        );
    }

    /// The behavioral difference from `deepagent_mcp::config::expand`, which
    /// turns unknown variables into empty strings. §9.2 requires the literal.
    #[test]
    fn unrecognized_placeholder_stays_literal() {
        assert_eq!(
            expand_v1("prefix-${MISSING}-suffix", ROOT, DATA),
            "prefix-${MISSING}-suffix"
        );
        assert_eq!(expand_v1("${API_TOKEN}", ROOT, DATA), "${API_TOKEN}");
        assert_eq!(
            expand_v1("${PLUGIN_ROOT}/${NOPE}", ROOT, DATA),
            "/plugins/demo/${NOPE}"
        );
    }

    /// Case matters: only the exact portable names expand.
    #[test]
    fn variable_names_are_case_sensitive() {
        assert_eq!(expand_v1("${plugin_root}", ROOT, DATA), "${plugin_root}");
        assert_eq!(expand_v1("${Plugin_Root}", ROOT, DATA), "${Plugin_Root}");
    }

    /// "Text introduced by a replacement MUST NOT be scanned for further
    /// placeholders." A root whose own path literally contains placeholder text
    /// is the case that proves it.
    #[test]
    fn replacement_text_is_not_rescanned() {
        let tricky_root = "/a/${PLUGIN_DATA}/b";
        assert_eq!(
            expand_v1("${PLUGIN_ROOT}/x", tricky_root, DATA),
            "/a/${PLUGIN_DATA}/b/x"
        );
    }

    #[test]
    fn preserves_unterminated_and_bare_dollar() {
        assert_eq!(expand_v1("${PLUGIN_ROOT", ROOT, DATA), "${PLUGIN_ROOT");
        assert_eq!(expand_v1("a ${ b", ROOT, DATA), "a ${ b");
        assert_eq!(expand_v1("$PLUGIN_ROOT", ROOT, DATA), "$PLUGIN_ROOT");
        assert_eq!(expand_v1("100$", ROOT, DATA), "100$");
        assert_eq!(expand_v1("${}", ROOT, DATA), "${}");
    }

    #[test]
    fn passes_through_text_without_placeholders() {
        assert_eq!(expand_v1("", ROOT, DATA), "");
        assert_eq!(expand_v1("--flag=value", ROOT, DATA), "--flag=value");
    }

    #[test]
    fn rewrites_claude_and_codex_aliases() {
        assert_eq!(
            rewrite_dialect_aliases("${CLAUDE_PLUGIN_ROOT}/hooks/pre.py"),
            "${PLUGIN_ROOT}/hooks/pre.py"
        );
        assert_eq!(
            rewrite_dialect_aliases("${CLAUDE_PLUGIN_DATA}/state"),
            "${PLUGIN_DATA}/state"
        );
        assert_eq!(
            rewrite_dialect_aliases("${CODEX_PLUGIN_ROOT}/bin"),
            "${PLUGIN_ROOT}/bin"
        );
    }

    #[test]
    fn alias_rewrite_leaves_other_variables_alone() {
        assert_eq!(rewrite_dialect_aliases("${API_TOKEN}"), "${API_TOKEN}");
        assert_eq!(rewrite_dialect_aliases("${PLUGIN_ROOT}"), "${PLUGIN_ROOT}");
        assert_eq!(rewrite_dialect_aliases("${UNCLOSED"), "${UNCLOSED");
    }

    /// The real shape found in Anthropic's bundled `hooks.json` files.
    #[test]
    fn normalize_and_expand_handles_claude_hook_command() {
        assert_eq!(
            normalize_and_expand(
                "python3 ${CLAUDE_PLUGIN_ROOT}/hooks/pretooluse.py",
                ROOT,
                DATA
            ),
            "python3 /plugins/demo/hooks/pretooluse.py"
        );
    }

    #[test]
    fn detects_reserved_env_keys() {
        let mut env = BTreeMap::new();
        env.insert("SAFE".to_string(), "1".to_string());
        assert_eq!(reserved_env_key(&env), None);

        env.insert(PLUGIN_ROOT_VAR.to_string(), "/anywhere".to_string());
        assert_eq!(reserved_env_key(&env), Some(PLUGIN_ROOT_VAR));

        let mut data_only = BTreeMap::new();
        data_only.insert(PLUGIN_DATA_VAR.to_string(), "/anywhere".to_string());
        assert_eq!(reserved_env_key(&data_only), Some(PLUGIN_DATA_VAR));
    }

    #[test]
    fn classifies_the_three_permitted_cwd_forms() {
        assert_eq!(
            classify_cwd("./data"),
            Some(CwdForm::PluginRelative("./data"))
        );
        assert_eq!(
            classify_cwd("${PLUGIN_ROOT}"),
            Some(CwdForm::PluginRoot { tail: "" })
        );
        assert_eq!(
            classify_cwd("${PLUGIN_ROOT}/sub"),
            Some(CwdForm::PluginRoot { tail: "sub" })
        );
        assert_eq!(
            classify_cwd("${PLUGIN_DATA}"),
            Some(CwdForm::PluginData { tail: "" })
        );
        assert_eq!(
            classify_cwd("${PLUGIN_DATA}/cache"),
            Some(CwdForm::PluginData { tail: "cache" })
        );
    }

    #[test]
    fn rejects_other_cwd_shapes() {
        for raw in [
            "data",            // not plugin-relative
            "../escape",       // no `./` prefix
            "/absolute",       // absolute
            "${PLUGIN_ROOT}x", // no separator after the variable
        ] {
            assert_eq!(classify_cwd(raw), None, "expected {raw:?} to be rejected");
        }
    }
}
