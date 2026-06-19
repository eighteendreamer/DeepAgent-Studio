//! Call-reference name matching.
//!
//! Turns parked call [`UnresolvedRef`]s into cross-file `calls` edges by
//! matching the referenced name against callable definition nodes (functions
//! and methods) already present in the graph.
//!
//! Matching is deliberately a *name-level* heuristic: the extractors emit a
//! best-effort callee name per call site (a simple name like `foo`, or a
//! qualified name like `Type::method` / `module.func`), and this matcher picks
//! the most plausible definition with that name. When more than one definition
//! survives disambiguation the chosen edge is flagged
//! `provenance = "heuristic"` so downstream consumers know it was inferred
//! rather than precisely bound. References with no candidate are simply left
//! unresolved — no error, no edge.

use std::collections::{BTreeMap, BTreeSet};

use crate::types::{Edge, EdgeKind, Node, NodeKind, UnresolvedRef};

/// `reference_kind` value the extractors use for call sites.
pub const CALL_REFERENCE_KIND: &str = "call";

/// Provenance marker stamped on heuristically-resolved `calls` edges.
pub const HEURISTIC_PROVENANCE: &str = "heuristic";

/// Index of callable definition nodes keyed by their simple `name`.
///
/// Only [`NodeKind::Function`] and [`NodeKind::Method`] nodes are indexed;
/// everything else is ignored. Built once per resolution pass and queried per
/// call reference.
#[derive(Debug, Default)]
pub struct DefinitionIndex {
    by_name: BTreeMap<String, Vec<Node>>,
}

impl DefinitionIndex {
    /// Build an index over the callable nodes in `nodes`.
    pub fn build<'a, I>(nodes: I) -> Self
    where
        I: IntoIterator<Item = &'a Node>,
    {
        let mut by_name: BTreeMap<String, Vec<Node>> = BTreeMap::new();
        for node in nodes {
            if is_callable(node.kind) {
                by_name
                    .entry(node.name.clone())
                    .or_default()
                    .push(node.clone());
            }
        }
        Self { by_name }
    }

    fn candidates(&self, simple_name: &str) -> &[Node] {
        self.by_name
            .get(simple_name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// Outcome of matching a batch of call references.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallMatchReport {
    /// `calls` edges produced (already de-duplicated within this batch).
    pub edges: Vec<Edge>,
    /// Number of call references that matched a definition.
    pub resolved: usize,
    /// Number of call references with no matching definition.
    pub unresolved: usize,
}

/// Match every call reference in `refs` against `index`, producing `calls`
/// edges.
///
/// Non-call references (`reference_kind != "call"`) are ignored. Edges are
/// de-duplicated by `(source, target, line)` within the batch so repeated
/// call sites on the same line never emit duplicates.
pub fn match_calls(refs: &[UnresolvedRef], index: &DefinitionIndex) -> CallMatchReport {
    let mut report = CallMatchReport::default();
    let mut seen: BTreeSet<(String, String, u32)> = BTreeSet::new();

    for reference in refs {
        if reference.reference_kind != CALL_REFERENCE_KIND {
            continue;
        }
        match resolve_call(reference, index) {
            Some((target, heuristic)) => {
                report.resolved += 1;
                let key = (
                    reference.from_node_id.clone(),
                    target.id.clone(),
                    reference.line,
                );
                if seen.insert(key) {
                    report.edges.push(Edge {
                        source: reference.from_node_id.clone(),
                        target: target.id.clone(),
                        kind: EdgeKind::Calls,
                        metadata: None,
                        line: Some(reference.line),
                        provenance: heuristic.then(|| HEURISTIC_PROVENANCE.to_string()),
                    });
                }
            }
            None => report.unresolved += 1,
        }
    }
    report
}

/// Resolve a single call reference to its best-ranked definition.
///
/// Returns the chosen node and whether the resolution was heuristic (more than
/// one candidate survived disambiguation). `None` when no definition carries
/// the referenced name.
fn resolve_call<'a>(
    reference: &UnresolvedRef,
    index: &'a DefinitionIndex,
) -> Option<(&'a Node, bool)> {
    let (simple, qualified) = split_reference(&reference.reference_name);
    let candidates = index.candidates(simple);
    if candidates.is_empty() {
        return None;
    }

    // When the reference is qualified (`Type::method`), prefer candidates whose
    // qualified name matches it; fall back to all same-name candidates if none
    // line up so we still resolve the call rather than dropping it.
    let pool: Vec<&Node> = match qualified.as_deref() {
        Some(q) => {
            let exact: Vec<&Node> = candidates
                .iter()
                .filter(|n| qualified_matches(&n.qualified_name, q))
                .collect();
            if exact.is_empty() {
                candidates.iter().collect()
            } else {
                exact
            }
        }
        None => candidates.iter().collect(),
    };

    let qualified_ref = qualified.as_deref();
    let best = pool.iter().copied().min_by(|a, b| {
        rank(a, reference, qualified_ref)
            .cmp(&rank(b, reference, qualified_ref))
            .then_with(|| a.id.cmp(&b.id))
    })?;

    // A unique surviving candidate is treated as a precise resolution; an
    // ambiguous pick (multiple candidates) is flagged heuristic.
    let heuristic = pool.len() > 1;
    Some((best, heuristic))
}

/// Priority rank for a candidate against a reference; lower is better.
///
/// Mirrors the design ordering: same file > same module/dir > qualified-name
/// exact match > everything else.
fn rank(node: &Node, reference: &UnresolvedRef, qualified_ref: Option<&str>) -> u8 {
    if node.file_path == reference.file_path {
        0
    } else if same_dir(&node.file_path, &reference.file_path) {
        1
    } else if qualified_ref
        .map(|q| qualified_matches(&node.qualified_name, q))
        .unwrap_or(false)
    {
        2
    } else {
        3
    }
}

fn is_callable(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Function | NodeKind::Method)
}

/// Split a referenced name into its simple (trailing) segment and, if the name
/// is qualified, the full qualified form.
///
/// Both `::` (Rust/TS) and `.` (Python/JS member access) are treated as path
/// separators.
fn split_reference(name: &str) -> (&str, Option<String>) {
    let has_separator = name.contains("::") || name.contains('.');
    let simple = name
        .rsplit([':', '.'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(name);
    if has_separator {
        (simple, Some(name.to_string()))
    } else {
        (name, None)
    }
}

/// Whether a candidate's qualified name satisfies a qualified reference.
///
/// Separators are normalised (`.` -> `::`) before comparison; the candidate
/// matches when it equals the reference or ends with `::{reference}` (so
/// `impl Foo for Bar::run` matches a `Bar::run` reference).
fn qualified_matches(candidate_qualified: &str, reference_qualified: &str) -> bool {
    let candidate = normalize_separators(candidate_qualified);
    let reference = normalize_separators(reference_qualified);
    candidate == reference || candidate.ends_with(&format!("::{reference}"))
}

fn normalize_separators(s: &str) -> String {
    s.replace('.', "::")
}

fn same_dir(a: &str, b: &str) -> bool {
    parent_dir(a) == parent_dir(b)
}

fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[..index],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Language;

    fn def(kind: NodeKind, name: &str, qualified: &str, file: &str, line: u32) -> Node {
        Node {
            id: format!("{}:{file}:{qualified}:{line}", kind.as_str()),
            kind,
            name: name.to_string(),
            qualified_name: qualified.to_string(),
            file_path: file.to_string(),
            language: Language::Rust,
            start_line: line,
            end_line: line + 5,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    fn call_ref(from: &str, name: &str, file: &str, line: u32) -> UnresolvedRef {
        UnresolvedRef {
            from_node_id: from.to_string(),
            reference_name: name.to_string(),
            reference_kind: CALL_REFERENCE_KIND.to_string(),
            line,
            file_path: file.to_string(),
        }
    }

    #[test]
    fn single_candidate_resolves_precisely() {
        let target = def(NodeKind::Function, "helper", "helper", "src/util.rs", 5);
        let index = DefinitionIndex::build(std::slice::from_ref(&target));
        let refs = vec![call_ref("caller", "helper", "src/main.rs", 12)];

        let report = match_calls(&refs, &index);

        assert_eq!(report.resolved, 1);
        assert_eq!(report.unresolved, 0);
        assert_eq!(report.edges.len(), 1);
        let edge = &report.edges[0];
        assert_eq!(edge.source, "caller");
        assert_eq!(edge.target, target.id);
        assert_eq!(edge.kind, EdgeKind::Calls);
        assert_eq!(edge.line, Some(12));
        // Unique resolution is precise, not heuristic.
        assert_eq!(edge.provenance, None);
    }

    #[test]
    fn same_file_candidate_wins_over_other_files() {
        let same_file = def(NodeKind::Function, "run", "run", "src/main.rs", 30);
        let other = def(NodeKind::Function, "run", "run", "src/other.rs", 4);
        // Order other first to prove ranking, not insertion order, decides.
        let index = DefinitionIndex::build(&[other.clone(), same_file.clone()]);
        let refs = vec![call_ref("caller", "run", "src/main.rs", 10)];

        let report = match_calls(&refs, &index);

        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.edges[0].target, same_file.id);
        // Multiple candidates -> heuristic.
        assert_eq!(
            report.edges[0].provenance.as_deref(),
            Some(HEURISTIC_PROVENANCE)
        );
    }

    #[test]
    fn qualified_reference_matches_qualified_name() {
        let method = def(NodeKind::Method, "method", "Type::method", "src/a.rs", 8);
        let decoy = def(NodeKind::Method, "method", "Other::method", "src/b.rs", 8);
        let index = DefinitionIndex::build(&[decoy, method.clone()]);
        let refs = vec![call_ref("caller", "Type::method", "src/c.rs", 3)];

        let report = match_calls(&refs, &index);

        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.edges[0].target, method.id);
        // Qualified match disambiguated to a single candidate -> precise.
        assert_eq!(report.edges[0].provenance, None);
    }

    #[test]
    fn dotted_qualified_reference_matches_double_colon_name() {
        let method = def(NodeKind::Method, "func", "module::func", "src/mod.py", 8);
        let index = DefinitionIndex::build(std::slice::from_ref(&method));
        let refs = vec![call_ref("caller", "module.func", "src/app.py", 3)];

        let report = match_calls(&refs, &index);

        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.edges[0].target, method.id);
    }

    #[test]
    fn cross_file_call_connects() {
        let target = def(NodeKind::Function, "compute", "compute", "src/b.rs", 2);
        let index = DefinitionIndex::build(std::slice::from_ref(&target));
        let refs = vec![call_ref(
            "function:src/a.rs:driver:1",
            "compute",
            "src/a.rs",
            4,
        )];

        let report = match_calls(&refs, &index);

        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.edges[0].source, "function:src/a.rs:driver:1");
        assert_eq!(report.edges[0].target, target.id);
    }

    #[test]
    fn no_candidate_leaves_reference_unresolved() {
        let unrelated = def(NodeKind::Function, "other", "other", "src/x.rs", 1);
        let index = DefinitionIndex::build(&[unrelated]);
        let refs = vec![call_ref("caller", "missing", "src/main.rs", 9)];

        let report = match_calls(&refs, &index);

        assert!(report.edges.is_empty());
        assert_eq!(report.resolved, 0);
        assert_eq!(report.unresolved, 1);
    }

    #[test]
    fn same_directory_ranks_above_distant_files() {
        let same_dir = def(NodeKind::Function, "load", "load", "src/db/util.rs", 12);
        let distant = def(NodeKind::Function, "load", "load", "src/ui/view.rs", 12);
        let index = DefinitionIndex::build(&[distant, same_dir.clone()]);
        let refs = vec![call_ref("caller", "load", "src/db/repo.rs", 7)];

        let report = match_calls(&refs, &index);

        assert_eq!(report.edges[0].target, same_dir.id);
    }

    #[test]
    fn duplicate_call_sites_on_same_line_are_deduped() {
        let target = def(NodeKind::Function, "helper", "helper", "src/util.rs", 5);
        let index = DefinitionIndex::build(std::slice::from_ref(&target));
        let refs = vec![
            call_ref("caller", "helper", "src/main.rs", 12),
            call_ref("caller", "helper", "src/main.rs", 12),
        ];

        let report = match_calls(&refs, &index);

        // Both references resolve, but only one edge is emitted.
        assert_eq!(report.resolved, 2);
        assert_eq!(report.edges.len(), 1);
    }

    #[test]
    fn non_call_references_are_ignored() {
        let target = def(NodeKind::Function, "helper", "helper", "src/util.rs", 5);
        let index = DefinitionIndex::build(&[target]);
        let refs = vec![UnresolvedRef {
            from_node_id: "caller".to_string(),
            reference_name: "helper".to_string(),
            reference_kind: "import".to_string(),
            line: 1,
            file_path: "src/main.rs".to_string(),
        }];

        let report = match_calls(&refs, &index);

        assert!(report.edges.is_empty());
        assert_eq!(report.resolved, 0);
        assert_eq!(report.unresolved, 0);
    }
}
