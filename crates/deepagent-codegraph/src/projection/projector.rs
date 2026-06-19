//! `Projector`: down-projects the rich SQLite code graph into the existing UA
//! `knowledge-graph.json` consumed unchanged by the front-end project-map
//! panel.
//!
//! The rich graph carries symbol-level detail (calls/extends/implements edges,
//! many node kinds, signatures). The UA schema is intentionally coarse: a small
//! set of node `type`s, three edge kinds the panel understands
//! (`contains`/`imports`/`related`), per-node `complexity`, plus `layers` and a
//! guided `tour`. This module performs that lossy projection while preserving
//! the schema invariants the front-end relies on:
//!
//! - node ids are unique,
//! - every edge `source`/`target` references a projected node id,
//! - `filePath` is a POSIX relative path,
//! - the top-level object carries `version` / `project` / `nodes` / `edges` /
//!   `layers` / `tour`.
//!
//! See `design.md` (Projector, UA JSON 投影格式) and the reference
//! `understand-anything` `types.ts` / `graph-builder.ts` for the target schema.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use deepagent_core::error::{CoreError, Result};

use crate::projection::layers::{build_layers, Layer};
use crate::projection::tour::{generate_tour, TourStep};
use crate::store::GraphStore;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};

/// UA schema version emitted at the top of `knowledge-graph.json`.
const UA_VERSION: &str = "1.0.0";

/// Line-count thresholds for the three UA complexity buckets.
const COMPLEX_LINES: u32 = 500;
const MODERATE_LINES: u32 = 150;

/// Project metadata block of the UA graph (`project` field).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UaProject {
    pub name: String,
    pub description: String,
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub analyzed_at: String,
    pub git_commit_hash: String,
}

/// A projected UA node (front-end `GraphNode`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UaNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_range: Option<[u32; 2]>,
    pub summary: String,
    pub tags: Vec<String>,
    pub complexity: String,
}

/// A projected UA edge (front-end `GraphEdge`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UaEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub direction: String,
    pub weight: f64,
}

/// The full UA `knowledge-graph.json` document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UaGraph {
    pub version: String,
    pub project: UaProject,
    pub nodes: Vec<UaNode>,
    pub edges: Vec<UaEdge>,
    pub layers: Vec<Layer>,
    pub tour: Vec<TourStep>,
}

/// Summary of a projection run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionStats {
    pub nodes: usize,
    pub edges: usize,
    pub layers: usize,
    pub tour_steps: usize,
    pub out_path: PathBuf,
}

/// Down-projects the rich graph held in a [`GraphStore`] into UA JSON.
pub struct Projector<'a> {
    store: &'a GraphStore,
}

impl<'a> Projector<'a> {
    /// Borrow a store to project from.
    pub fn new(store: &'a GraphStore) -> Self {
        Self { store }
    }

    /// Build the UA graph and write it to `out_path` (creating parent dirs).
    ///
    /// `project_root` is scanned for a manifest (Cargo.toml / package.json /
    /// pyproject.toml / go.mod) to fill in project name, description and
    /// frameworks. Returns counts plus the path written.
    pub fn project(&self, project_root: &Path, out_path: &Path) -> Result<ProjectionStats> {
        let graph = self.build_graph(project_root)?;
        let stats = ProjectionStats {
            nodes: graph.nodes.len(),
            edges: graph.edges.len(),
            layers: graph.layers.len(),
            tour_steps: graph.tour.len(),
            out_path: out_path.to_path_buf(),
        };

        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    CoreError::Persistence(format!(
                        "failed to create projection directory {}: {e}",
                        parent.display()
                    ))
                })?;
            }
        }
        let json = serde_json::to_string_pretty(&graph)?;
        fs::write(out_path, json).map_err(|e| {
            CoreError::Persistence(format!(
                "failed to write projection {}: {e}",
                out_path.display()
            ))
        })?;

        Ok(stats)
    }

    /// Build the in-memory UA graph from the store (no file I/O for nodes/edges).
    ///
    /// Exposed within the crate so tests can assert on structure without
    /// touching the filesystem.
    pub fn build_graph(&self, project_root: &Path) -> Result<UaGraph> {
        let rich_nodes = self.store.all_nodes()?;
        let rich_edges = self.store.all_edges()?;

        let nodes: Vec<UaNode> = rich_nodes.iter().map(project_node).collect();
        let node_ids: BTreeSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let edges = project_edges(&rich_edges, &node_ids);

        // Layers and tour are computed over the rich nodes/edges so the
        // heuristics see the original edge kinds (e.g. imports for topo order).
        let layers = build_layers(&rich_nodes);
        let tour = generate_tour(&rich_nodes, &rich_edges, &layers);

        let project = self.build_project_meta(project_root, &rich_nodes)?;

        Ok(UaGraph {
            version: UA_VERSION.to_string(),
            project,
            nodes,
            edges,
            layers,
            tour,
        })
    }

    /// Assemble the `project` metadata block from manifests + git metadata.
    fn build_project_meta(&self, project_root: &Path, nodes: &[Node]) -> Result<UaProject> {
        let manifest = ManifestInfo::scan(project_root);
        let name = manifest.name.clone().unwrap_or_else(|| {
            project_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string()
        });
        let git_commit_hash = self
            .store
            .get_metadata("git_commit_hash")?
            .unwrap_or_default();

        Ok(UaProject {
            name,
            description: manifest.description.unwrap_or_default(),
            languages: project_languages(nodes),
            frameworks: manifest.frameworks,
            analyzed_at: iso8601_utc_now(),
            git_commit_hash,
        })
    }
}

/// Map a rich [`Node`] to a UA node, computing type/complexity/summary.
fn project_node(node: &Node) -> UaNode {
    UaNode {
        id: node.id.clone(),
        node_type: ua_node_type(node.kind).to_string(),
        name: node.name.clone(),
        file_path: if node.file_path.is_empty() {
            None
        } else {
            Some(node.file_path.clone())
        },
        line_range: Some([node.start_line, node.end_line]),
        summary: node_summary(node),
        tags: node_tags(node),
        complexity: complexity_for_lines(node.start_line, node.end_line).to_string(),
    }
}

/// Map a [`NodeKind`] to the coarse UA node `type` string.
///
/// The UA front-end only understands a fixed `NodeType` set. Symbol kinds that
/// have no exact UA counterpart are folded to the closest supported value,
/// following the reference `understand-anything` conventions (e.g. data-like
/// members → `config`, routes → `endpoint`).
fn ua_node_type(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::File => "file",
        NodeKind::Module | NodeKind::Namespace => "module",
        NodeKind::Class
        | NodeKind::Struct
        | NodeKind::Interface
        | NodeKind::Trait
        | NodeKind::Enum
        | NodeKind::TypeAlias => "class",
        NodeKind::Function | NodeKind::Method => "function",
        NodeKind::Property
        | NodeKind::Field
        | NodeKind::Variable
        | NodeKind::Constant
        | NodeKind::EnumMember
        | NodeKind::Import => "config",
        NodeKind::Route => "endpoint",
    }
}

/// Compute the UA complexity bucket from an inclusive line span.
///
/// Length is `end_line - start_line + 1`; `>= 500` is `complex`, `>= 150` is
/// `moderate`, otherwise `simple`. Guards against `end < start`.
fn complexity_for_lines(start_line: u32, end_line: u32) -> &'static str {
    let lines = end_line.saturating_sub(start_line).saturating_add(1);
    if lines >= COMPLEX_LINES {
        "complex"
    } else if lines >= MODERATE_LINES {
        "moderate"
    } else {
        "simple"
    }
}

/// Produce a deterministic, non-LLM summary for a node.
fn node_summary(node: &Node) -> String {
    if let Some(doc) = node.docstring.as_ref() {
        if let Some(line) = doc.lines().map(str::trim).find(|l| !l.is_empty()) {
            return line.to_string();
        }
    }
    if let Some(sig) = node.signature.as_ref() {
        let sig = sig.trim();
        if !sig.is_empty() {
            return sig.to_string();
        }
    }
    let kind = node.kind.as_str().replace('_', " ");
    if node.name.is_empty() {
        kind
    } else {
        format!("{kind} {}", node.name)
    }
}

/// Tags for a node: its language plus a `changed` marker when present in the
/// rich graph (left for incremental sync to populate).
fn node_tags(node: &Node) -> Vec<String> {
    let mut tags = Vec::new();
    if node.language != Language::Other {
        tags.push(node.language.as_str().to_string());
    }
    tags
}

/// Down-project rich edges to the UA edge set, dropping edges whose endpoints
/// are not projected and de-duplicating on `(source, target, type)`.
fn project_edges(edges: &[Edge], node_ids: &BTreeSet<&str>) -> Vec<UaEdge> {
    let mut seen: BTreeSet<(String, String, &'static str)> = BTreeSet::new();
    let mut out = Vec::new();
    for edge in edges {
        if !node_ids.contains(edge.source.as_str()) || !node_ids.contains(edge.target.as_str()) {
            continue;
        }
        let edge_type = ua_edge_type(edge.kind);
        let key = (edge.source.clone(), edge.target.clone(), edge_type);
        if !seen.insert(key) {
            continue;
        }
        out.push(UaEdge {
            source: edge.source.clone(),
            target: edge.target.clone(),
            edge_type: edge_type.to_string(),
            direction: "forward".to_string(),
            weight: ua_edge_weight(edge_type),
        });
    }
    out
}

/// Map an [`EdgeKind`] to a UA edge `type`.
///
/// Only `contains` and `imports` survive verbatim; every other rich edge kind
/// (calls/extends/implements/references/exports/type_of) is folded to the
/// generic `related` so the projection never emits a type the panel cannot
/// render.
fn ua_edge_type(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Contains => "contains",
        EdgeKind::Imports => "imports",
        EdgeKind::Calls
        | EdgeKind::Exports
        | EdgeKind::Extends
        | EdgeKind::Implements
        | EdgeKind::References
        | EdgeKind::TypeOf => "related",
    }
}

/// Heuristic weight for a UA edge type (0..=1).
fn ua_edge_weight(edge_type: &str) -> f64 {
    match edge_type {
        "contains" => 1.0,
        "imports" => 0.7,
        _ => 0.5,
    }
}

/// Distinct, sorted language display strings across the supplied nodes,
/// excluding [`Language::Other`].
fn project_languages(nodes: &[Node]) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for node in nodes {
        if node.language != Language::Other {
            set.insert(node.language.as_str().to_string());
        }
    }
    set.into_iter().collect()
}

/// Lightweight project-manifest facts used to fill the UA `project` block.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ManifestInfo {
    name: Option<String>,
    description: Option<String>,
    frameworks: Vec<String>,
}

/// Known framework/runtime tokens mapped to their display name. Matched
/// case-insensitively against manifest dependency text.
const FRAMEWORK_TOKENS: &[(&str, &str)] = &[
    ("react", "React"),
    ("next", "Next.js"),
    ("vue", "Vue"),
    ("svelte", "Svelte"),
    ("angular", "Angular"),
    ("express", "Express"),
    ("fastify", "Fastify"),
    ("nestjs", "NestJS"),
    ("@nestjs/core", "NestJS"),
    ("koa", "Koa"),
    ("tauri", "Tauri"),
    ("django", "Django"),
    ("flask", "Flask"),
    ("fastapi", "FastAPI"),
    ("axum", "Axum"),
    ("actix-web", "Actix"),
    ("rocket", "Rocket"),
    ("warp", "Warp"),
    ("tokio", "Tokio"),
    ("gin-gonic", "Gin"),
    ("gin", "Gin"),
    ("echo", "Echo"),
    ("fiber", "Fiber"),
];

impl ManifestInfo {
    /// Scan well-known manifests at `project_root`, merging what is found.
    fn scan(project_root: &Path) -> Self {
        let mut info = ManifestInfo::default();

        if let Some(content) = read_manifest(project_root, "package.json") {
            info.merge(parse_package_json(&content));
        }
        if let Some(content) = read_manifest(project_root, "Cargo.toml") {
            info.merge(parse_cargo_toml(&content));
        }
        if let Some(content) = read_manifest(project_root, "pyproject.toml") {
            info.merge(parse_pyproject_toml(&content));
        }
        if let Some(content) = read_manifest(project_root, "go.mod") {
            info.merge(parse_go_mod(&content));
        }

        info.frameworks.sort();
        info.frameworks.dedup();
        info
    }

    /// Fill empty name/description from `other`; union frameworks.
    fn merge(&mut self, other: ManifestInfo) {
        if self.name.is_none() {
            self.name = other.name;
        }
        if self.description.is_none() {
            self.description = other.description;
        }
        self.frameworks.extend(other.frameworks);
    }
}

/// Read a manifest file's contents, returning `None` if missing/unreadable.
fn read_manifest(project_root: &Path, file_name: &str) -> Option<String> {
    fs::read_to_string(project_root.join(file_name)).ok()
}

/// Scan free-form dependency text for known framework tokens.
fn detect_frameworks(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    for (token, display) in FRAMEWORK_TOKENS {
        if contains_dependency_token(&lower, token) && !found.contains(&display.to_string()) {
            found.push(display.to_string());
        }
    }
    found
}

/// True if `token` appears in `text` bounded by non-identifier characters, so
/// `gin` does not match `imaging`.
fn contains_dependency_token(text: &str, token: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = text[start..].find(token) {
        let abs = start + pos;
        let before = text[..abs].chars().next_back();
        let after = text[abs + token.len()..].chars().next();
        let boundary = |c: Option<char>| {
            c.map(|c| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(true)
        };
        // `-`, `@`, `/`, `.` are part of package names; treat them as part of
        // the token so `actix-web` matches but `gingival` does not match `gin`.
        let token_has_seps = token.contains(['-', '@', '/', '.']);
        let before_ok =
            boundary(before) || (token_has_seps && matches!(before, Some('-' | '@' | '/' | '.')));
        let after_ok = boundary(after);
        if before_ok && after_ok {
            return true;
        }
        start = abs + token.len();
    }
    false
}

/// Parse `package.json` for name, description and framework dependencies.
fn parse_package_json(content: &str) -> ManifestInfo {
    let mut info = ManifestInfo::default();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return info;
    };
    info.name = value
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    info.description = value
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut dep_keys = String::new();
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(obj) = value.get(section).and_then(|v| v.as_object()) {
            for key in obj.keys() {
                dep_keys.push_str(key);
                dep_keys.push('\n');
            }
        }
    }
    info.frameworks = detect_frameworks(&dep_keys);
    info
}

/// Parse `Cargo.toml` for `[package] name/description` and dependency tokens.
fn parse_cargo_toml(content: &str) -> ManifestInfo {
    let mut info = ManifestInfo::default();
    let mut section = String::new();
    let mut deps_text = String::new();
    for raw in content.lines() {
        let line = strip_toml_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.trim().to_string();
            continue;
        }
        if section == "package" {
            if let Some(value) = toml_string_value(line, "name") {
                info.name.get_or_insert(value);
            } else if let Some(value) = toml_string_value(line, "description") {
                info.description.get_or_insert(value);
            }
        }
        if section == "dependencies" || section.ends_with("dependencies") {
            if let Some((key, _)) = line.split_once('=') {
                deps_text.push_str(key.trim());
                deps_text.push('\n');
            }
        }
    }
    info.frameworks = detect_frameworks(&deps_text);
    info
}

/// Parse `pyproject.toml` for project name/description (PEP 621 and poetry).
fn parse_pyproject_toml(content: &str) -> ManifestInfo {
    let mut info = ManifestInfo::default();
    let mut section = String::new();
    let mut deps_text = String::new();
    for raw in content.lines() {
        let line = strip_toml_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.trim().to_string();
            continue;
        }
        if section == "project" || section == "tool.poetry" {
            if let Some(value) = toml_string_value(line, "name") {
                info.name.get_or_insert(value);
            } else if let Some(value) = toml_string_value(line, "description") {
                info.description.get_or_insert(value);
            }
        }
        deps_text.push_str(line);
        deps_text.push('\n');
    }
    info.frameworks = detect_frameworks(&deps_text);
    info
}

/// Parse `go.mod`: the module path's last segment becomes the project name.
fn parse_go_mod(content: &str) -> ManifestInfo {
    let mut info = ManifestInfo::default();
    let mut deps_text = String::new();
    for raw in content.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("module ") {
            let path = rest.trim();
            let name = path.rsplit('/').next().unwrap_or(path);
            if !name.is_empty() {
                info.name.get_or_insert(name.to_string());
            }
        } else {
            deps_text.push_str(line);
            deps_text.push('\n');
        }
    }
    info.frameworks = detect_frameworks(&deps_text);
    info
}

/// Strip a trailing `# ...` comment from a TOML line (no quote awareness; good
/// enough for the simple name/description lines we read).
fn strip_toml_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// Extract `key = "value"` from a TOML line, returning the unquoted value.
fn toml_string_value(line: &str, key: &str) -> Option<String> {
    let (lhs, rhs) = line.split_once('=')?;
    if lhs.trim() != key {
        return None;
    }
    let value = rhs.trim();
    let unquoted = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
    Some(unquoted.to_string())
}

/// Current UTC time formatted as `YYYY-MM-DDTHH:MM:SSZ` (ISO 8601).
fn iso8601_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    iso8601_utc(secs)
}

/// Format Unix epoch seconds as an ISO 8601 UTC timestamp.
///
/// Uses Howard Hinnant's civil-from-days algorithm so no date crate is needed.
fn iso8601_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };

    format!("{year:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};

    fn node(id: &str, kind: NodeKind, file_path: &str, start: u32, end: u32) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: id.rsplit(':').next().unwrap_or(id).to_string(),
            qualified_name: id.to_string(),
            file_path: file_path.to_string(),
            language: Language::Rust,
            start_line: start,
            end_line: end,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    fn edge(source: &str, target: &str, kind: EdgeKind) -> Edge {
        Edge {
            source: source.to_string(),
            target: target.to_string(),
            kind,
            metadata: None,
            line: None,
            provenance: None,
        }
    }

    #[test]
    fn complexity_thresholds_are_inclusive() {
        // 1..=149 lines -> simple, 150..=499 -> moderate, >=500 -> complex.
        assert_eq!(complexity_for_lines(1, 1), "simple");
        assert_eq!(complexity_for_lines(1, 149), "simple");
        assert_eq!(complexity_for_lines(1, 150), "moderate");
        assert_eq!(complexity_for_lines(10, 159), "moderate"); // 150 lines
        assert_eq!(complexity_for_lines(1, 499), "moderate");
        assert_eq!(complexity_for_lines(1, 500), "complex");
        assert_eq!(complexity_for_lines(100, 700), "complex");
    }

    #[test]
    fn complexity_handles_inverted_span() {
        assert_eq!(complexity_for_lines(10, 5), "simple");
    }

    #[test]
    fn node_kind_maps_to_supported_ua_types() {
        const SUPPORTED: &[&str] = &[
            "file", "function", "class", "module", "concept", "config", "document", "service",
            "table", "endpoint", "pipeline", "schema", "resource",
        ];
        for kind in [
            NodeKind::File,
            NodeKind::Module,
            NodeKind::Namespace,
            NodeKind::Class,
            NodeKind::Struct,
            NodeKind::Interface,
            NodeKind::Trait,
            NodeKind::Enum,
            NodeKind::TypeAlias,
            NodeKind::Function,
            NodeKind::Method,
            NodeKind::Property,
            NodeKind::Field,
            NodeKind::Variable,
            NodeKind::Constant,
            NodeKind::EnumMember,
            NodeKind::Import,
            NodeKind::Route,
        ] {
            let ty = ua_node_type(kind);
            assert!(SUPPORTED.contains(&ty), "{kind:?} -> {ty} not supported");
        }
        assert_eq!(ua_node_type(NodeKind::File), "file");
        assert_eq!(ua_node_type(NodeKind::Function), "function");
        assert_eq!(ua_node_type(NodeKind::Method), "function");
        assert_eq!(ua_node_type(NodeKind::Class), "class");
        assert_eq!(ua_node_type(NodeKind::Struct), "class");
        assert_eq!(ua_node_type(NodeKind::Module), "module");
        assert_eq!(ua_node_type(NodeKind::Constant), "config");
        assert_eq!(ua_node_type(NodeKind::Route), "endpoint");
    }

    #[test]
    fn edge_kinds_project_to_panel_supported_types() {
        const PANEL: &[&str] = &["contains", "imports", "related"];
        for kind in [
            EdgeKind::Contains,
            EdgeKind::Imports,
            EdgeKind::Calls,
            EdgeKind::Exports,
            EdgeKind::Extends,
            EdgeKind::Implements,
            EdgeKind::References,
            EdgeKind::TypeOf,
        ] {
            assert!(PANEL.contains(&ua_edge_type(kind)));
        }
        assert_eq!(ua_edge_type(EdgeKind::Contains), "contains");
        assert_eq!(ua_edge_type(EdgeKind::Imports), "imports");
        assert_eq!(ua_edge_type(EdgeKind::Calls), "related");
        assert_eq!(ua_edge_type(EdgeKind::Extends), "related");
        assert_eq!(ua_edge_type(EdgeKind::Implements), "related");
    }

    #[test]
    fn project_edges_drops_dangling_and_dedups() {
        let ids: BTreeSet<&str> = ["a", "b"].into_iter().collect();
        let edges = vec![
            edge("a", "b", EdgeKind::Calls),
            edge("a", "b", EdgeKind::Extends), // both -> related, dedup with above
            edge("a", "missing", EdgeKind::Calls), // dangling target dropped
            edge("a", "b", EdgeKind::Contains),
        ];
        let projected = project_edges(&edges, &ids);
        assert_eq!(projected.len(), 2);
        let types: BTreeSet<&str> = projected.iter().map(|e| e.edge_type.as_str()).collect();
        assert!(types.contains("related"));
        assert!(types.contains("contains"));
        assert!(projected.iter().all(|e| e.direction == "forward"));
    }

    #[test]
    fn iso8601_formats_known_instants() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        // 2021-01-01T00:00:00Z
        assert_eq!(iso8601_utc(1_609_459_200), "2021-01-01T00:00:00Z");
        // 2023-03-14T15:09:26Z
        assert_eq!(iso8601_utc(1_678_806_566), "2023-03-14T15:09:26Z");
    }

    #[test]
    fn dependency_token_matching_respects_boundaries() {
        assert!(contains_dependency_token("react\nreact-dom\n", "react"));
        assert!(contains_dependency_token("actix-web\n", "actix-web"));
        assert!(!contains_dependency_token("imaging\n", "gin"));
        assert!(contains_dependency_token("gin\n", "gin"));
    }

    #[test]
    fn parses_cargo_and_package_manifests() {
        let cargo = parse_cargo_toml(
            "[package]\nname = \"my-crate\"\ndescription = \"A tool\"\n\n[dependencies]\naxum = \"0.7\"\ntokio = { version = \"1\" }\n",
        );
        assert_eq!(cargo.name.as_deref(), Some("my-crate"));
        assert_eq!(cargo.description.as_deref(), Some("A tool"));
        assert!(cargo.frameworks.contains(&"Axum".to_string()));
        assert!(cargo.frameworks.contains(&"Tokio".to_string()));

        let pkg = parse_package_json(
            r#"{ "name": "web-app", "description": "front end", "dependencies": { "react": "^18", "express": "^4" } }"#,
        );
        assert_eq!(pkg.name.as_deref(), Some("web-app"));
        assert!(pkg.frameworks.contains(&"React".to_string()));
        assert!(pkg.frameworks.contains(&"Express".to_string()));

        let go = parse_go_mod("module github.com/acme/widget\n\ngo 1.21\n");
        assert_eq!(go.name.as_deref(), Some("widget"));

        let py = parse_pyproject_toml("[project]\nname = \"svc\"\ndescription = \"py svc\"\n");
        assert_eq!(py.name.as_deref(), Some("svc"));
        assert_eq!(py.description.as_deref(), Some("py svc"));
    }

    #[test]
    fn build_graph_from_in_memory_store_matches_schema() {
        let store = GraphStore::open_in_memory().unwrap();
        store.set_metadata("git_commit_hash", "deadbeef").unwrap();

        let nodes = vec![
            node(
                "file:src/api/users.rs",
                NodeKind::File,
                "src/api/users.rs",
                1,
                200,
            ),
            node(
                "function:src/api/users.rs:handler:10",
                NodeKind::Function,
                "src/api/users.rs",
                10,
                620,
            ),
            node(
                "function:src/api/users.rs:helper:640",
                NodeKind::Function,
                "src/api/users.rs",
                640,
                660,
            ),
            node(
                "file:src/util/mod.rs",
                NodeKind::File,
                "src/util/mod.rs",
                1,
                30,
            ),
        ];
        store.insert_nodes(&nodes).unwrap();
        store
            .insert_edges(&[
                edge(
                    "file:src/api/users.rs",
                    "function:src/api/users.rs:handler:10",
                    EdgeKind::Contains,
                ),
                edge(
                    "file:src/api/users.rs",
                    "file:src/util/mod.rs",
                    EdgeKind::Imports,
                ),
                // calls edge between two real symbols -> down-projected to "related".
                edge(
                    "function:src/api/users.rs:handler:10",
                    "function:src/api/users.rs:helper:640",
                    EdgeKind::Calls,
                ),
            ])
            .unwrap();

        let projector = Projector::new(&store);
        let graph = projector.build_graph(Path::new("/tmp/project")).unwrap();

        // Top-level schema fields.
        assert_eq!(graph.version, "1.0.0");
        assert_eq!(graph.project.git_commit_hash, "deadbeef");
        assert!(graph.project.languages.contains(&"rust".to_string()));
        assert!(graph.project.analyzed_at.ends_with('Z'));

        // Node id uniqueness.
        let ids: BTreeSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids.len(), graph.nodes.len());
        assert_eq!(graph.nodes.len(), 4);

        // Complexity computed from line spans.
        let handler = graph
            .nodes
            .iter()
            .find(|n| n.id == "function:src/api/users.rs:handler:10")
            .unwrap();
        assert_eq!(handler.complexity, "complex"); // 611 lines
        assert_eq!(handler.node_type, "function");
        let util = graph
            .nodes
            .iter()
            .find(|n| n.id == "file:src/util/mod.rs")
            .unwrap();
        assert_eq!(util.complexity, "simple");

        // Every edge references a valid, projected node and uses POSIX paths.
        for e in &graph.edges {
            assert!(ids.contains(e.source.as_str()));
            assert!(ids.contains(e.target.as_str()));
        }
        // contains + imports kept verbatim; calls -> related.
        assert_eq!(graph.edges.len(), 3);
        assert!(graph.edges.iter().any(|e| e.edge_type == "contains"));
        assert!(graph.edges.iter().any(|e| e.edge_type == "imports"));
        assert!(graph.edges.iter().any(|e| e.edge_type == "related"));
        assert!(graph.edges.iter().all(|e| e.edge_type != "calls"));

        // POSIX file paths.
        for n in &graph.nodes {
            if let Some(fp) = &n.file_path {
                assert!(!fp.contains('\\'), "file path should be POSIX: {fp}");
            }
        }

        // Layers + tour present and non-trivial.
        assert!(!graph.layers.is_empty());
        assert!(!graph.tour.is_empty());
        assert_eq!(graph.tour.first().unwrap().order, 1);
    }

    #[test]
    fn project_writes_file_and_reports_stats() {
        let store = GraphStore::open_in_memory().unwrap();
        store
            .insert_nodes(&[node(
                "file:src/main.rs",
                NodeKind::File,
                "src/main.rs",
                1,
                10,
            )])
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join(".understand-anything/knowledge-graph.json");
        let projector = Projector::new(&store);
        let stats = projector.project(dir.path(), &out).unwrap();

        assert_eq!(stats.nodes, 1);
        assert!(out.exists());

        let written = std::fs::read_to_string(&out).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["version"], "1.0.0");
        assert!(parsed["project"].is_object());
        assert!(parsed["nodes"].is_array());
        assert!(parsed["edges"].is_array());
        assert!(parsed["layers"].is_array());
        assert!(parsed["tour"].is_array());
        // The single file node carries the required UA fields.
        let first = &parsed["nodes"][0];
        assert_eq!(first["type"], "file");
        assert!(first["summary"].is_string());
        assert!(first["tags"].is_array());
        assert!(first["complexity"].is_string());
    }
}
