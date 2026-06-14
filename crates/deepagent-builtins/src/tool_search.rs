//! Lazy tool loading via the `tool_search` channel.
//!
//! See `.kiro/specs/tool-search/` for the full spec. This module owns:
//!
//! - [`ToolSearchMode`] — user-facing tri-state (`Disabled` / `Enabled` /
//!   `Auto`) that decides whether deferral is active for a session.
//! - [`is_deferred_tool`] — the stateless policy function that combines the
//!   mode with each tool's `should_defer` / `always_load` metadata.
//! - [`ToolSearchTool`] (Phase 2A — added in a later task) — the built-in
//!   tool the model invokes to discover deferred tools.
//!
//! ## Policy summary
//!
//! ```text
//! mode == Disabled                             → never defer
//! tool.always_load() == true                   → never defer
//! tool.descriptor().name == "tool_search"      → never defer (must always be reachable)
//! mode == Enabled                              → defer iff tool.should_defer()
//! mode == Auto                                 → defer iff tool.should_defer()
//!                                                (caller still has to pass an
//!                                                 above-threshold check before
//!                                                 actually treating it as deferred)
//! ```

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::Result;
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};
/// The reserved tool name for the discovery channel itself. Hard-coded here
/// (instead of being read off a tool instance) so the policy stays callable
/// without instantiating `ToolSearchTool`.
pub const TOOL_SEARCH_TOOL_NAME: &str = "tool_search";

/// User-selectable lazy-tool-loading mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolSearchMode {
    /// All tools are loaded into every request. Backward-compatible default.
    #[default]
    Disabled,
    /// Deferrable tools are hidden until discovered via `tool_search`.
    Enabled,
    /// Deferral activates only when the deferrable tools' total schema size
    /// exceeds the auto-threshold (computed by the caller).
    Auto,
}

impl ToolSearchMode {
    /// Stable wire / UI label.
    pub const fn label(&self) -> &'static str {
        match self {
            ToolSearchMode::Disabled => "disabled",
            ToolSearchMode::Enabled => "enabled",
            ToolSearchMode::Auto => "auto",
        }
    }

    /// Parse from a wire string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "disabled" => Some(Self::Disabled),
            "enabled" => Some(Self::Enabled),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    /// Whether this mode runs the deferral pipeline at all (above-threshold
    /// checks for `Auto` happen separately at the call site).
    pub fn is_active(&self) -> bool {
        !matches!(self, ToolSearchMode::Disabled)
    }
}

/// Whether `tool` should be treated as deferred under `mode`.
///
/// Stateless: the function takes only the tool's own metadata + the mode.
/// Threshold checks for `Auto` are deliberately NOT in here — they need the
/// full tool list and the runtime's char/token estimator, which belong at the
/// chat-service layer.
pub fn is_deferred_tool(tool: &dyn Tool, mode: ToolSearchMode) -> bool {
    if !mode.is_active() {
        return false;
    }
    if tool.always_load() {
        return false;
    }
    if tool.descriptor().name == TOOL_SEARCH_TOOL_NAME {
        return false;
    }
    tool.should_defer()
}

/// Default `max_results` when the model omits the field.
pub const TOOL_SEARCH_DEFAULT_MAX_RESULTS: usize = 5;

/// Hard cap on `max_results` (we never return more, regardless of input).
pub const TOOL_SEARCH_MAX_RESULTS_CAP: usize = 20;

/// A frozen view of one deferred tool that [`ToolSearchTool`] can rank.
///
/// Captured at construction time from the registry: the search side of the
/// loop only needs name + description. Schema / permissions / risk live in
/// the registry itself; once a tool gets discovered, the chat-service layer
/// re-fetches the live descriptor so any post-construction registry update
/// is honored when the tool is actually called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredToolSnapshot {
    /// Wire name of the tool (matches `ToolDescriptor::name`).
    pub name: String,
    /// One-line description used for keyword-search ranking.
    pub description: String,
}

/// The `tool_search` built-in. Lets the model discover deferred tools by
/// either explicit selection (`select:Name1,Name2`) or keyword search.
pub struct ToolSearchTool {
    /// Snapshot of every deferrable tool's name + description, captured at
    /// construction.
    deferred: Vec<DeferredToolSnapshot>,
    /// Shared discovered-set handle for the owning session. Successful
    /// matches are added here so the next chat turn can include the tool
    /// schema in the model's `tools` array.
    discovered: Arc<Mutex<HashSet<String>>>,
}

impl ToolSearchTool {
    /// Build over a deferred-tool snapshot and the session's discovered-set
    /// handle.
    pub fn new(
        deferred: Vec<DeferredToolSnapshot>,
        discovered: Arc<Mutex<HashSet<String>>>,
    ) -> Self {
        Self {
            deferred,
            discovered,
        }
    }

    /// Total number of deferred tools known at construction time.
    pub fn deferred_tool_count(&self) -> usize {
        self.deferred.len()
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: TOOL_SEARCH_TOOL_NAME.to_string(),
            description: "Discover and load deferred tools so they can be called.\n\
                \n\
                Some tools (especially MCP server tools) are NOT loaded into your initial tool list — only their names appear, in the `<available-deferred-tools>` block. Use this tool to fetch their full schemas. Once a tool's schema lands, you can call it on the next turn exactly like any built-in.\n\
                \n\
                ## Args\n\
                - `query` (string, required) — what to discover.\n\
                - `max_results` (int, optional, default 5, max 20) — only used by keyword search.\n\
                \n\
                ## Three query syntaxes\n\
                \n\
                **Direct selection** — `select:Name1,Name2,Name3`\n\
                Pass the exact tool names you want. Found names are loaded; missing names are reported back in `missing`.\n\
                \n\
                **Keyword search** — `notebook jupyter`\n\
                Free-text query. Each whitespace-separated term is matched against tool name parts (snake / camelCase / `mcp__server__action` are all split correctly) and descriptions. Results are ranked by name match > description match. Returns up to `max_results`.\n\
                \n\
                **Required terms** — `+slack send message`\n\
                A `+`-prefixed term is REQUIRED — candidates that don't contain it are filtered out before ranking. Use this when you know one term is unambiguous (e.g. server name) and want the rest to disambiguate within that subset.\n\
                \n\
                ## Result\n\
                ```json\n\
                {\n\
                  \"matches\": [{\"name\": \"...\", \"description\": \"...\"}],\n\
                  \"query\": \"<original query>\",\n\
                  \"total_deferred_tools\": <int>,\n\
                  \"missing\": [\"...\"]   // only present when select: had unknown names\n\
                }\n\
                ```\n\
                An empty `matches` array is a normal `ok` result, not a failure — it just means nothing matched.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "select:Name1,Name2 OR free-text keywords OR +required keyword phrase."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": TOOL_SEARCH_MAX_RESULTS_CAP,
                        "default": TOOL_SEARCH_DEFAULT_MAX_RESULTS,
                        "description": "Cap on results for keyword search; ignored by select: queries."
                    }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(query_raw) = args.get("query").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'query'"));
        };
        let query = query_raw.trim();
        if query.is_empty() {
            return Ok(ToolOutput::failure("'query' must not be empty"));
        }
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(TOOL_SEARCH_DEFAULT_MAX_RESULTS)
            .clamp(1, TOOL_SEARCH_MAX_RESULTS_CAP);

        // Direct selection: `select:Name1,Name2`. Names are matched
        // case-sensitively against the deferred snapshot. Missing names go
        // into `missing` so the model knows which ones to reconsider.
        if let Some(rest) = query.strip_prefix("select:") {
            let names: Vec<&str> = rest
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            let mut matches: Vec<&DeferredToolSnapshot> = Vec::new();
            let mut missing: Vec<String> = Vec::new();
            for name in &names {
                if let Some(snap) = self.deferred.iter().find(|s| s.name == *name) {
                    if !matches.iter().any(|m| m.name == snap.name) {
                        matches.push(snap);
                    }
                } else {
                    missing.push((*name).to_string());
                }
            }
            self.add_to_discovered(matches.iter().map(|m| m.name.clone()));
            return Ok(ToolOutput::success(build_result_value(
                matches.iter().map(|m| (*m).clone()).collect(),
                query_raw,
                self.deferred.len(),
                if missing.is_empty() {
                    None
                } else {
                    Some(missing)
                },
            )));
        }

        // Keyword search.
        let (required, optional) = split_required_terms(query);
        let mut scored: Vec<(u32, &DeferredToolSnapshot)> = self
            .deferred
            .iter()
            .filter_map(|snap| score_tool(snap, &required, &optional).map(|score| (score, snap)))
            .collect();
        // Higher score first; break ties by alphabetical name for determinism.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        let matches: Vec<DeferredToolSnapshot> = scored
            .into_iter()
            .take(max_results)
            .map(|(_, s)| s.clone())
            .collect();
        self.add_to_discovered(matches.iter().map(|m| m.name.clone()));
        Ok(ToolOutput::success(build_result_value(
            matches,
            query_raw,
            self.deferred.len(),
            None,
        )))
    }

    fn always_load(&self) -> bool {
        // The discovery channel can never be deferred behind itself — the
        // model needs it on every turn to fetch other deferred tools.
        true
    }
}

impl ToolSearchTool {
    fn add_to_discovered<I: IntoIterator<Item = String>>(&self, names: I) {
        let mut set = self.discovered.lock().unwrap_or_else(|p| p.into_inner());
        for name in names {
            set.insert(name);
        }
    }
}

/// Split a free-text query into (required terms, optional terms).
///
/// `+foo bar baz` → required=["foo"], optional=["bar","baz"]. Terms are
/// lowercased and de-duplicated within each bucket. Empty inputs yield
/// empty buckets.
fn split_required_terms(query: &str) -> (Vec<String>, Vec<String>) {
    let mut required = Vec::new();
    let mut optional = Vec::new();
    for raw in query.split_whitespace() {
        if let Some(rest) = raw.strip_prefix('+') {
            if !rest.is_empty() {
                let lower = rest.to_lowercase();
                if !required.contains(&lower) {
                    required.push(lower);
                }
            }
        } else if !raw.is_empty() {
            let lower = raw.to_lowercase();
            if !optional.contains(&lower) {
                optional.push(lower);
            }
        }
    }
    (required, optional)
}

/// Split a tool name into search-friendly word parts. Handles three forms:
/// - MCP namespaced: `mcp__server__action_name` → `["server", "action", "name"]`
/// - snake_case: `read_file` → `["read", "file"]`
/// - CamelCase: `WebFetch` → `["web", "fetch"]`
///
/// All parts are lowercased and empty parts are filtered.
pub fn parse_tool_name(name: &str) -> Vec<String> {
    let stripped = name.strip_prefix("mcp__").unwrap_or(name);
    let mut parts: Vec<String> = Vec::new();
    for chunk in stripped.split("__") {
        // Insert a separator at every lower→upper boundary so CamelCase
        // splits into individual words alongside any snake separators.
        let mut buf = String::with_capacity(chunk.len() + 4);
        let mut prev_lower = false;
        for c in chunk.chars() {
            if c.is_uppercase() && prev_lower {
                buf.push(' ');
            }
            buf.push(c);
            prev_lower = c.is_lowercase();
        }
        for token in buf.split(|c: char| c == '_' || c.is_whitespace()) {
            if !token.is_empty() {
                parts.push(token.to_lowercase());
            }
        }
    }
    parts
}

/// Score one deferred tool against a search query. Returns `None` when any
/// required term is missing from both name and description (the candidate is
/// disqualified).
///
/// Weights (mirroring Claude Code's ranking heuristics):
/// - exact name-part match: +10 (or +12 if MCP)
/// - substring match within a name part: +5 (or +6 if MCP)
/// - description full-name fallback: +3
/// - description word-boundary match: +2
pub fn score_tool(
    snap: &DeferredToolSnapshot,
    required: &[String],
    optional: &[String],
) -> Option<u32> {
    let parts = parse_tool_name(&snap.name);
    let full = parts.join(" ");
    let desc_lower = snap.description.to_lowercase();
    let is_mcp = snap.name.starts_with("mcp__");

    // Disqualify candidates that don't contain every required term in either
    // their name parts (any granularity) or their description.
    for term in required {
        let term_in_parts = parts.iter().any(|p| p == term || p.contains(term));
        let term_in_full = full.contains(term);
        let term_in_desc = description_contains_word(&desc_lower, term);
        if !(term_in_parts || term_in_full || term_in_desc) {
            return None;
        }
    }

    // Score against every term (required + optional). Required terms still
    // contribute to score so a candidate that matches both the required
    // anchor AND optional disambiguators outranks one that only matches the
    // anchor.
    let all: Vec<&String> = required.iter().chain(optional.iter()).collect();
    if all.is_empty() {
        // Empty query (no terms after parsing): no candidates ranked.
        return None;
    }

    let mut score: u32 = 0;
    for term in &all {
        let mut term_score: u32 = 0;
        // Tier 1: exact name-part match.
        if parts.iter().any(|p| p == *term) {
            term_score += if is_mcp { 12 } else { 10 };
        } else if parts.iter().any(|p| p.contains(term.as_str())) {
            // Tier 2: substring match inside a name part.
            term_score += if is_mcp { 6 } else { 5 };
        } else if full.contains(term.as_str()) {
            // Tier 3: full-name fallback.
            term_score += 3;
        }
        // Tier 4: description word-boundary match (additive — name match +
        // description match both contribute).
        if description_contains_word(&desc_lower, term) {
            term_score += 2;
        }
        score = score.saturating_add(term_score);
    }

    if score == 0 {
        None
    } else {
        Some(score)
    }
}

/// Check whether `desc_lower` (already lowercased) contains `term` at a word
/// boundary. Word boundary = start of string, end of string, or a non-
/// alphanumeric character on either side.
fn description_contains_word(desc_lower: &str, term: &str) -> bool {
    if term.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(idx) = desc_lower[start..].find(term) {
        let abs = start + idx;
        let before_ok = abs == 0
            || !desc_lower
                .as_bytes()
                .get(abs - 1)
                .copied()
                .map(|b| (b as char).is_alphanumeric())
                .unwrap_or(false);
        let after_idx = abs + term.len();
        let after_ok = after_idx >= desc_lower.len()
            || !desc_lower
                .as_bytes()
                .get(after_idx)
                .copied()
                .map(|b| (b as char).is_alphanumeric())
                .unwrap_or(false);
        if before_ok && after_ok {
            return true;
        }
        start = abs + term.len();
    }
    false
}

fn build_result_value(
    matches: Vec<DeferredToolSnapshot>,
    query: &str,
    total_deferred: usize,
    missing: Option<Vec<String>>,
) -> serde_json::Value {
    let matches_json: Vec<serde_json::Value> = matches
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "name": m.name,
                "description": m.description,
            })
        })
        .collect();
    let mut value = serde_json::json!({
        "matches": matches_json,
        "query": query,
        "total_deferred_tools": total_deferred,
    });
    if let Some(missing) = missing {
        value["missing"] =
            serde_json::Value::Array(missing.into_iter().map(serde_json::Value::String).collect());
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use deepagent_core::error::Result;
    use deepagent_tools::permission::{PermissionSet, RiskLevel};
    use deepagent_tools::{ToolDescriptor, ToolOutput};

    /// Stub tool whose `should_defer`, `always_load`, and `name` are all
    /// configurable so we can exercise every combination of the policy.
    struct StubTool {
        name: &'static str,
        should_defer: bool,
        always_load: bool,
    }

    #[async_trait]
    impl Tool for StubTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.name.to_string(),
                description: "stub".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }
        async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
            Ok(ToolOutput::success(serde_json::json!(null)))
        }
        fn should_defer(&self) -> bool {
            self.should_defer
        }
        fn always_load(&self) -> bool {
            self.always_load
        }
    }

    fn stub(name: &'static str, should_defer: bool, always_load: bool) -> StubTool {
        StubTool {
            name,
            should_defer,
            always_load,
        }
    }

    // ----- ToolSearchMode -----

    #[test]
    fn mode_default_is_disabled() {
        assert_eq!(ToolSearchMode::default(), ToolSearchMode::Disabled);
    }

    #[test]
    fn mode_label_round_trips() {
        for m in [
            ToolSearchMode::Disabled,
            ToolSearchMode::Enabled,
            ToolSearchMode::Auto,
        ] {
            assert_eq!(ToolSearchMode::parse(m.label()), Some(m));
        }
        assert_eq!(ToolSearchMode::parse("nonsense"), None);
    }

    #[test]
    fn mode_is_active_only_for_non_disabled() {
        assert!(!ToolSearchMode::Disabled.is_active());
        assert!(ToolSearchMode::Enabled.is_active());
        assert!(ToolSearchMode::Auto.is_active());
    }

    #[test]
    fn mode_serde_round_trips_via_snake_case() {
        let v = serde_json::to_value(ToolSearchMode::Auto).unwrap();
        assert_eq!(v, serde_json::Value::String("auto".into()));
        let back: ToolSearchMode = serde_json::from_value(v).unwrap();
        assert_eq!(back, ToolSearchMode::Auto);
    }

    // ----- is_deferred_tool: truth table -----

    #[test]
    fn disabled_mode_never_defers() {
        // Disabled short-circuits regardless of tool flags or name.
        let t1 = stub("read_file", false, false);
        let t2 = stub("mcp__svc__list", true, false);
        let t3 = stub("tool_search", true, false);
        assert!(!is_deferred_tool(&t1, ToolSearchMode::Disabled));
        assert!(!is_deferred_tool(&t2, ToolSearchMode::Disabled));
        assert!(!is_deferred_tool(&t3, ToolSearchMode::Disabled));
    }

    #[test]
    fn always_load_overrides_should_defer() {
        // A tool flagged as always_load wins over should_defer.
        let t = stub("special", true, true);
        assert!(!is_deferred_tool(&t, ToolSearchMode::Enabled));
        assert!(!is_deferred_tool(&t, ToolSearchMode::Auto));
    }

    #[test]
    fn tool_search_itself_is_never_deferred() {
        // The discovery channel must always reach the model; it can never be
        // deferred behind itself.
        let t = stub(TOOL_SEARCH_TOOL_NAME, true, false);
        assert!(!is_deferred_tool(&t, ToolSearchMode::Enabled));
        assert!(!is_deferred_tool(&t, ToolSearchMode::Auto));
    }

    #[test]
    fn enabled_mode_defers_iff_should_defer() {
        let mcp_like = stub("mcp__svc__list", true, false);
        let builtin = stub("read_file", false, false);
        assert!(is_deferred_tool(&mcp_like, ToolSearchMode::Enabled));
        assert!(!is_deferred_tool(&builtin, ToolSearchMode::Enabled));
    }

    #[test]
    fn auto_mode_defers_iff_should_defer() {
        // Auto mode behaves like Enabled at this layer — the threshold gate
        // sits at the caller (chat_service). When the caller decides the
        // budget is below threshold it simply passes Disabled instead.
        let mcp_like = stub("mcp__svc__list", true, false);
        let builtin = stub("read_file", false, false);
        assert!(is_deferred_tool(&mcp_like, ToolSearchMode::Auto));
        assert!(!is_deferred_tool(&builtin, ToolSearchMode::Auto));
    }

    #[test]
    fn full_truth_table() {
        // Cross-product: 3 modes × 2 should_defer × 2 always_load × 2 tool_name buckets.
        for (mode_label, mode) in [
            ("Disabled", ToolSearchMode::Disabled),
            ("Enabled", ToolSearchMode::Enabled),
            ("Auto", ToolSearchMode::Auto),
        ] {
            for sd in [false, true] {
                for al in [false, true] {
                    for name in ["mcp__svc__list", TOOL_SEARCH_TOOL_NAME] {
                        let tool = stub(name, sd, al);
                        let got = is_deferred_tool(&tool, mode);
                        // Defer iff: mode is active AND not always_load AND
                        // name isn't tool_search itself AND should_defer is true.
                        let expected =
                            mode.is_active() && !al && name != TOOL_SEARCH_TOOL_NAME && sd;
                        assert_eq!(
                            got, expected,
                            "mode={mode_label} should_defer={sd} always_load={al} name={name}: \
                             expected {expected}, got {got}"
                        );
                    }
                }
            }
        }
    }

    // ----- parse_tool_name -----

    #[test]
    fn parse_tool_name_handles_snake_case() {
        assert_eq!(parse_tool_name("read_file"), vec!["read", "file"]);
        assert_eq!(parse_tool_name("multi_edit"), vec!["multi", "edit"]);
    }

    #[test]
    fn parse_tool_name_handles_camel_case() {
        assert_eq!(parse_tool_name("WebFetch"), vec!["web", "fetch"]);
        assert_eq!(
            parse_tool_name("TaskListTool"),
            vec!["task", "list", "tool"]
        );
    }

    #[test]
    fn parse_tool_name_handles_mcp_prefix() {
        assert_eq!(
            parse_tool_name("mcp__slack__send_message"),
            vec!["slack", "send", "message"]
        );
        assert_eq!(
            parse_tool_name("mcp__github__list_repos"),
            vec!["github", "list", "repos"]
        );
    }

    #[test]
    fn parse_tool_name_combines_camel_and_snake() {
        // Hybrid `camelCase_snake` like Claude Code's `BatchTool_run` would.
        assert_eq!(
            parse_tool_name("BatchTool_run"),
            vec!["batch", "tool", "run"]
        );
    }

    // ----- description_contains_word -----

    #[test]
    fn description_word_boundary_matches() {
        assert!(description_contains_word("send a slack message", "slack"));
        assert!(description_contains_word("send a slack message", "send"));
        // Substring without word boundary doesn't match.
        assert!(!description_contains_word("blackjack", "lack"));
        // Punctuation counts as non-alphanumeric → word boundary.
        assert!(description_contains_word("search,grep,find files", "grep"));
    }

    // ----- score_tool -----

    fn snap(name: &str, description: &str) -> DeferredToolSnapshot {
        DeferredToolSnapshot {
            name: name.to_string(),
            description: description.to_string(),
        }
    }

    #[test]
    fn score_returns_none_when_required_term_missing() {
        let s = snap("mcp__github__list_repos", "list github repositories");
        let required = vec!["slack".to_string()];
        let optional = vec!["list".to_string()];
        assert_eq!(score_tool(&s, &required, &optional), None);
    }

    #[test]
    fn score_returns_some_when_all_required_present() {
        let s = snap("mcp__slack__send_message", "send a Slack message");
        let required = vec!["slack".to_string()];
        let optional = vec!["send".to_string()];
        assert!(score_tool(&s, &required, &optional).is_some());
    }

    #[test]
    fn score_ranks_exact_part_match_above_substring() {
        let exact = snap("mcp__slack__send", "send something");
        let substring = snap("mcp__send_email__service", "email a slack-like target");
        let sc_exact = score_tool(&exact, &[], &["slack".to_string()]).unwrap();
        let sc_sub = score_tool(&substring, &[], &["slack".to_string()]).unwrap();
        assert!(
            sc_exact > sc_sub,
            "exact part match ({sc_exact}) should beat substring match ({sc_sub})"
        );
    }

    #[test]
    fn score_mcp_bonus_outranks_non_mcp_match() {
        // MCP tools get a +2 bonus per matching tier so they surface before
        // built-ins with the same logical match (matches Claude Code's design
        // where MCP server names are high-signal).
        let mcp = snap("mcp__slack__send", "send a message");
        let non_mcp = snap("slack_send", "send a message");
        let sc_mcp = score_tool(&mcp, &[], &["slack".to_string()]).unwrap();
        let sc_non = score_tool(&non_mcp, &[], &["slack".to_string()]).unwrap();
        assert!(sc_mcp > sc_non);
    }

    #[test]
    fn score_returns_none_for_empty_query() {
        let s = snap("mcp__slack__send", "send");
        assert_eq!(score_tool(&s, &[], &[]), None);
    }

    // ----- split_required_terms -----

    #[test]
    fn split_required_terms_partitions_correctly() {
        let (req, opt) = split_required_terms("+slack send notebook");
        assert_eq!(req, vec!["slack"]);
        assert_eq!(opt, vec!["send", "notebook"]);
    }

    #[test]
    fn split_required_terms_lowercases_and_dedups() {
        let (req, opt) = split_required_terms("+Slack +slack Send send");
        assert_eq!(req, vec!["slack"]);
        assert_eq!(opt, vec!["send"]);
    }

    #[test]
    fn split_required_terms_drops_empty_plus() {
        // Lone `+` (no body) is dropped silently.
        let (req, opt) = split_required_terms("+ search");
        assert!(req.is_empty());
        assert_eq!(opt, vec!["search"]);
    }

    // ----- ToolSearchTool::invoke -----

    fn build_tool(
        deferred: Vec<DeferredToolSnapshot>,
    ) -> (ToolSearchTool, Arc<Mutex<HashSet<String>>>) {
        let discovered = Arc::new(Mutex::new(HashSet::new()));
        let tool = ToolSearchTool::new(deferred, discovered.clone());
        (tool, discovered)
    }

    #[tokio::test]
    async fn select_loads_named_tools_into_discovered_set() {
        let (tool, discovered) = build_tool(vec![
            snap("read_file", "read a file"),
            snap("bash", "run a command"),
            snap("mcp__slack__send", "send a slack message"),
        ]);
        let out = tool
            .invoke(serde_json::json!({"query": "select:bash,mcp__slack__send"}))
            .await
            .unwrap();
        assert!(out.ok);
        let names: Vec<&str> = out.value["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["bash", "mcp__slack__send"]);
        let set = discovered.lock().unwrap();
        assert!(set.contains("bash"));
        assert!(set.contains("mcp__slack__send"));
        assert!(!set.contains("read_file"));
    }

    #[tokio::test]
    async fn select_reports_missing_names_without_failing() {
        let (tool, discovered) = build_tool(vec![snap("read_file", "read a file")]);
        let out = tool
            .invoke(serde_json::json!({"query": "select:read_file,ghost"}))
            .await
            .unwrap();
        assert!(out.ok);
        let missing: Vec<&str> = out.value["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(missing, vec!["ghost"]);
        // Found names still got added.
        assert!(discovered.lock().unwrap().contains("read_file"));
    }

    #[tokio::test]
    async fn keyword_search_ranks_relevant_tools_first() {
        let (tool, _discovered) = build_tool(vec![
            snap("mcp__slack__send", "send a slack message"),
            snap("mcp__slack__list", "list slack channels"),
            snap("mcp__github__list", "list github repos"),
            snap("read_file", "read a file"),
        ]);
        let out = tool
            .invoke(serde_json::json!({"query": "slack send"}))
            .await
            .unwrap();
        let names: Vec<&str> = out.value["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        // mcp__slack__send must be first (matches both terms in name parts).
        assert_eq!(names[0], "mcp__slack__send");
        // mcp__github__list shouldn't appear at all (no slack and no send).
        assert!(!names.contains(&"mcp__github__list"));
    }

    #[tokio::test]
    async fn keyword_search_required_terms_filter_out_non_matching() {
        let (tool, _) = build_tool(vec![
            snap("mcp__slack__send", "send a slack message"),
            snap("mcp__github__send", "send a github comment"),
        ]);
        let out = tool
            .invoke(serde_json::json!({"query": "+slack send"}))
            .await
            .unwrap();
        let names: Vec<&str> = out.value["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["mcp__slack__send"]);
    }

    #[tokio::test]
    async fn keyword_search_clamps_max_results() {
        let many: Vec<DeferredToolSnapshot> = (0..30)
            .map(|i| snap(&format!("mcp__svc__op_{i}"), "operate on something"))
            .collect();
        let (tool, _) = build_tool(many);
        let out = tool
            .invoke(serde_json::json!({"query": "operate", "max_results": 999}))
            .await
            .unwrap();
        let count = out.value["matches"].as_array().unwrap().len();
        assert!(
            count <= TOOL_SEARCH_MAX_RESULTS_CAP,
            "got {count} matches, expected <= {TOOL_SEARCH_MAX_RESULTS_CAP}"
        );
    }

    #[tokio::test]
    async fn keyword_search_empty_match_is_ok_not_failure() {
        let (tool, _) = build_tool(vec![snap("read_file", "read a file")]);
        let out = tool
            .invoke(serde_json::json!({"query": "totally unrelated"}))
            .await
            .unwrap();
        // Empty match must NOT flip ok — it's a normal "no hit" outcome.
        assert!(out.ok);
        assert_eq!(out.value["matches"].as_array().unwrap().len(), 0);
        assert_eq!(out.value["total_deferred_tools"], 1);
    }

    #[tokio::test]
    async fn keyword_search_orders_deterministically_on_score_tie() {
        // Two tools with the same name-match score must come back in
        // alphabetical order so the model's experience is reproducible.
        let (tool, _) = build_tool(vec![
            snap("mcp__zeta__send", "send"),
            snap("mcp__alpha__send", "send"),
        ]);
        let out = tool
            .invoke(serde_json::json!({"query": "send"}))
            .await
            .unwrap();
        let names: Vec<&str> = out.value["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["mcp__alpha__send", "mcp__zeta__send"]);
    }

    #[tokio::test]
    async fn empty_query_fails() {
        let (tool, _) = build_tool(vec![snap("read_file", "")]);
        let out = tool
            .invoke(serde_json::json!({"query": "  "}))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn missing_query_fails() {
        let (tool, _) = build_tool(vec![]);
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn tool_search_tool_overrides_always_load() {
        let (tool, _) = build_tool(vec![]);
        assert!(tool.always_load());
        // And it's recognized as non-deferred even when its should_defer is
        // somehow forced (the hard-coded name check in is_deferred_tool wins).
        assert!(!is_deferred_tool(&tool, ToolSearchMode::Enabled));
        assert!(!is_deferred_tool(&tool, ToolSearchMode::Auto));
    }

    #[tokio::test]
    async fn discovered_set_accumulates_across_calls() {
        let (tool, discovered) = build_tool(vec![snap("a", ""), snap("b", ""), snap("c", "")]);
        tool.invoke(serde_json::json!({"query": "select:a"}))
            .await
            .unwrap();
        tool.invoke(serde_json::json!({"query": "select:b,c"}))
            .await
            .unwrap();
        let set = discovered.lock().unwrap();
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert!(set.contains("c"));
    }
}
