//! Project map service.
//!
//! It consumes an Understand-Anything project-map graph and can rebuild it
//! through the Understand-Anything core engine.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use deepagent_codegraph::CodeGraph;
use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

const UA_GRAPH: &str = ".understand-anything/knowledge-graph.json";
const MAX_STALE_CHECK_FILES: usize = 2500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapStatusDto {
    pub status: String,
    pub source: Option<String>,
    pub graph_path: Option<String>,
    pub updated_at: Option<i64>,
    pub nodes: usize,
    pub edges: usize,
    pub files: usize,
    pub functions: usize,
    pub classes: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapOverviewDto {
    pub status: ProjectMapStatusDto,
    pub project_name: Option<String>,
    pub description: Option<String>,
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub complex_nodes: Vec<ProjectMapHitDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapHitDto {
    pub node_id: String,
    pub node_type: String,
    pub name: String,
    pub file_path: Option<String>,
    pub summary: String,
    pub complexity: String,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapEdgeDto {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapGraphDto {
    pub nodes: Vec<ProjectMapHitDto>,
    pub edges: Vec<ProjectMapEdgeDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapNodeDto {
    pub id: String,
    pub node_type: String,
    pub name: String,
    pub file_path: Option<String>,
    pub line_range: Option<[u32; 2]>,
    pub summary: String,
    pub tags: Vec<String>,
    pub complexity: String,
    pub language_notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapNeighborDto {
    pub edge_type: String,
    pub direction: String,
    pub node: ProjectMapHitDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapNeighborsDto {
    pub node: Option<ProjectMapNodeDto>,
    pub imports: Vec<ProjectMapNeighborDto>,
    pub imported_by: Vec<ProjectMapNeighborDto>,
    pub calls: Vec<ProjectMapNeighborDto>,
    pub called_by: Vec<ProjectMapNeighborDto>,
    pub related: Vec<ProjectMapNeighborDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapImpactDto {
    pub target: Option<ProjectMapNodeDto>,
    pub direct: Vec<ProjectMapHitDto>,
    pub indirect: Vec<ProjectMapHitDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMapRefreshDto {
    pub ok: bool,
    pub graph_path: String,
    pub files: usize,
    pub nodes: usize,
    pub edges: usize,
    pub duration_ms: u64,
    pub truncated: bool,
    pub message: String,
    pub status: ProjectMapStatusDto,
}

#[derive(Debug, Clone, Deserialize)]
struct RawGraph {
    project: Option<RawProjectMeta>,
    #[serde(default)]
    nodes: Vec<RawNode>,
    #[serde(default)]
    edges: Vec<RawEdge>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProjectMeta {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    frameworks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawNode {
    id: String,
    #[serde(rename = "type")]
    node_type: String,
    name: String,
    #[serde(rename = "filePath")]
    file_path: Option<String>,
    #[serde(rename = "lineRange")]
    line_range: Option<[u32; 2]>,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_complexity")]
    complexity: String,
    #[serde(rename = "languageNotes")]
    language_notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawEdge {
    source: String,
    target: String,
    #[serde(rename = "type")]
    edge_type: String,
    #[serde(default, rename = "direction")]
    _direction: String,
    #[serde(default, rename = "weight")]
    _weight: f32,
}

fn default_complexity() -> String {
    "simple".to_string()
}

#[derive(Debug, Clone)]
struct LoadedGraph {
    source: String,
    project_root: PathBuf,
    graph_path: PathBuf,
    updated_at: Option<i64>,
    graph: RawGraph,
}

#[derive(Debug, Default)]
pub struct ProjectMapService;

impl ProjectMapService {
    pub fn new() -> Self {
        Self
    }

    pub fn status(&self, project_root: &Path) -> ProjectMapStatusDto {
        match self.load(project_root) {
            Ok(loaded) => self.ready_status(&loaded),
            Err(e) => missing_or_failed_status(project_root, Some(e.to_string())),
        }
    }

    pub fn overview(&self, project_root: &Path) -> ProjectMapOverviewDto {
        match self.load(project_root) {
            Ok(loaded) => {
                let status = self.ready_status(&loaded);
                let project = loaded.graph.project.clone();
                let complex_nodes = complex_hits(&loaded.graph, 10);
                ProjectMapOverviewDto {
                    status,
                    project_name: project.as_ref().and_then(|p| p.name.clone()),
                    description: project.as_ref().and_then(|p| p.description.clone()),
                    languages: project
                        .as_ref()
                        .map(|p| p.languages.clone())
                        .unwrap_or_default(),
                    frameworks: project
                        .as_ref()
                        .map(|p| p.frameworks.clone())
                        .unwrap_or_default(),
                    complex_nodes,
                }
            }
            Err(e) => ProjectMapOverviewDto {
                status: missing_or_failed_status(project_root, Some(e.to_string())),
                project_name: None,
                description: None,
                languages: Vec::new(),
                frameworks: Vec::new(),
                complex_nodes: Vec::new(),
            },
        }
    }

    pub fn search(
        &self,
        project_root: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ProjectMapHitDto>> {
        let loaded = self.load(project_root)?;
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(complex_hits(&loaded.graph, limit.clamp(1, 50)));
        }
        let mut hits = loaded
            .graph
            .nodes
            .iter()
            .filter_map(|node| {
                let hay = format!(
                    "{}\n{}\n{}\n{}\n{}",
                    node.name,
                    node.file_path.as_deref().unwrap_or(""),
                    node.summary,
                    node.tags.join(" "),
                    node.node_type
                )
                .to_lowercase();
                if !hay.contains(&q) {
                    return None;
                }
                let score = if node.name.to_lowercase().contains(&q) {
                    0.95
                } else if node
                    .file_path
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&q)
                {
                    0.85
                } else {
                    0.65
                };
                Some(hit_from_node(node, score))
            })
            .collect::<Vec<_>>();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        hits.truncate(limit.clamp(1, 50));
        Ok(hits)
    }

    pub fn node(&self, project_root: &Path, node_id: &str) -> Result<Option<ProjectMapNodeDto>> {
        let loaded = self.load(project_root)?;
        Ok(loaded
            .graph
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .map(node_dto))
    }

    pub fn neighbors(&self, project_root: &Path, node_id: &str) -> Result<ProjectMapNeighborsDto> {
        let loaded = self.load(project_root)?;
        Ok(neighbors_from_graph(&loaded.graph, node_id))
    }

    pub fn graph(&self, project_root: &Path, limit: usize) -> Result<ProjectMapGraphDto> {
        let loaded = self.load(project_root)?;
        Ok(graph_slice(&loaded.graph, limit.clamp(20, 160)))
    }

    pub fn impact(&self, project_root: &Path, node_ref: &str) -> Result<ProjectMapImpactDto> {
        let loaded = self.load(project_root)?;
        let Some(target) = resolve_node(&loaded.graph, node_ref) else {
            return Ok(ProjectMapImpactDto {
                target: None,
                direct: Vec::new(),
                indirect: Vec::new(),
            });
        };
        let by_id = node_map(&loaded.graph);
        let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &loaded.graph.edges {
            reverse
                .entry(edge.target.as_str())
                .or_default()
                .push(edge.source.as_str());
        }

        let mut seen = HashSet::new();
        let mut direct = Vec::new();
        let mut indirect = Vec::new();
        let mut queue = VecDeque::new();
        seen.insert(target.id.as_str());
        for dep in reverse.get(target.id.as_str()).cloned().unwrap_or_default() {
            if seen.insert(dep) {
                queue.push_back((dep, 1usize));
            }
        }
        while let Some((id, depth)) = queue.pop_front() {
            if let Some(node) = by_id.get(id) {
                if depth == 1 {
                    direct.push(hit_from_node(node, 1.0));
                } else if indirect.len() < 50 {
                    indirect.push(hit_from_node(node, 0.7));
                }
            }
            if depth >= 3 {
                continue;
            }
            for dep in reverse.get(id).cloned().unwrap_or_default() {
                if seen.insert(dep) {
                    queue.push_back((dep, depth + 1));
                }
            }
        }
        direct.truncate(50);
        Ok(ProjectMapImpactDto {
            target: Some(node_dto(target)),
            direct,
            indirect,
        })
    }

    pub fn refresh_deep(&self, project_root: &Path) -> Result<ProjectMapRefreshDto> {
        let start = Instant::now();
        let root = project_root
            .canonicalize()
            .map_err(|e| CoreError::invalid(format!("invalid project root: {e}")))?;
        if !root.is_dir() {
            return Err(CoreError::invalid(format!(
                "project root is not a directory: {}",
                root.display()
            )));
        }

        // Native code-graph engine: extract with tree-sitter into SQLite, then
        // project the UA knowledge-graph.json the front-end panel consumes. No
        // external Node.js process is involved.
        let mut graph = CodeGraph::open(&root)?;
        let index = if graph.has_existing_index() {
            graph.sync()?
        } else {
            graph.index_all()?
        };

        let graph_path = root.join(UA_GRAPH);
        let projection = graph.project_ua_json(&graph_path)?;

        let status = self.status(&root);
        let message = if index.is_incremental {
            format!(
                "Project map synced incrementally: {} file(s) re-indexed, {} nodes, {} edges.",
                index.files_indexed, projection.nodes, projection.edges
            )
        } else {
            format!(
                "Project map generated: {} files, {} nodes, {} edges.",
                index.files_indexed, projection.nodes, projection.edges
            )
        };

        Ok(ProjectMapRefreshDto {
            ok: true,
            graph_path: graph_path.to_string_lossy().to_string(),
            files: status.files,
            nodes: projection.nodes,
            edges: projection.edges,
            duration_ms: start.elapsed().as_millis() as u64,
            truncated: false,
            message,
            status,
        })
    }

    fn load(&self, project_root: &Path) -> Result<LoadedGraph> {
        let ua = project_root.join(UA_GRAPH);
        if ua.is_file() {
            return load_graph_file(project_root, "understand-anything", ua);
        }
        Err(CoreError::invalid(format!(
            "project map not found under {}",
            project_root.join(UA_GRAPH).display()
        )))
    }

    fn ready_status(&self, loaded: &LoadedGraph) -> ProjectMapStatusDto {
        let files = loaded
            .graph
            .nodes
            .iter()
            .filter(|n| is_file_level(&n.node_type))
            .count();
        let functions = loaded
            .graph
            .nodes
            .iter()
            .filter(|n| n.node_type == "function")
            .count();
        let classes = loaded
            .graph
            .nodes
            .iter()
            .filter(|n| n.node_type == "class")
            .count();
        let stale = loaded
            .updated_at
            .map(|graph_ms| has_newer_project_file(&loaded.project_root, graph_ms))
            .unwrap_or(false);
        ProjectMapStatusDto {
            status: if stale { "stale" } else { "ready" }.to_string(),
            source: Some(loaded.source.clone()),
            graph_path: Some(loaded.graph_path.to_string_lossy().to_string()),
            updated_at: loaded.updated_at,
            nodes: loaded.graph.nodes.len(),
            edges: loaded.graph.edges.len(),
            files,
            functions,
            classes,
            last_error: None,
        }
    }
}

fn load_graph_file(project_root: &Path, source: &str, graph_path: PathBuf) -> Result<LoadedGraph> {
    let updated_at = std::fs::metadata(&graph_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);
    let raw = std::fs::read_to_string(&graph_path)
        .map_err(|e| CoreError::Persistence(format!("read project map: {e}")))?;
    let graph: RawGraph = serde_json::from_str(&raw)
        .map_err(|e| CoreError::Persistence(format!("parse project map: {e}")))?;
    Ok(LoadedGraph {
        source: source.to_string(),
        project_root: project_root.to_path_buf(),
        graph_path,
        updated_at,
        graph,
    })
}

fn has_newer_project_file(project_root: &Path, graph_ms: i64) -> bool {
    fn visit(dir: &Path, graph_ms: i64, checked: &mut usize) -> bool {
        if *checked > MAX_STALE_CHECK_FILES {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if !should_skip_stale_dir(&name) && visit(&path, graph_ms, checked) {
                    return true;
                }
                continue;
            }
            if !path.is_file() || should_skip_stale_file(&name) {
                continue;
            }
            *checked += 1;
            let modified = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or_default();
            if modified > graph_ms {
                return true;
            }
        }
        false
    }

    let mut checked = 0;
    visit(project_root, graph_ms, &mut checked)
}

fn missing_or_failed_status(project_root: &Path, error: Option<String>) -> ProjectMapStatusDto {
    let has_candidate = project_root.join(UA_GRAPH).exists();
    ProjectMapStatusDto {
        status: if has_candidate { "failed" } else { "missing" }.to_string(),
        source: None,
        graph_path: None,
        updated_at: None,
        nodes: 0,
        edges: 0,
        files: 0,
        functions: 0,
        classes: 0,
        last_error: error,
    }
}

fn should_skip_stale_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".idea"
            | ".vscode"
            | ".deepagent"
            | ".understand-anything"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "out"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | ".cache"
            | "coverage"
            | "vendor"
            | "venv"
            | ".venv"
            | "__pycache__"
    )
}

fn should_skip_stale_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".lock")
        || lower.ends_with(".log")
        || lower.ends_with(".tmp")
        || lower.ends_with(".map")
}

fn is_file_level(node_type: &str) -> bool {
    matches!(
        node_type,
        "file"
            | "config"
            | "document"
            | "service"
            | "table"
            | "endpoint"
            | "pipeline"
            | "schema"
            | "resource"
    )
}

fn complexity_rank(c: &str) -> u8 {
    match c {
        "complex" => 3,
        "moderate" => 2,
        _ => 1,
    }
}

fn complex_hits(graph: &RawGraph, limit: usize) -> Vec<ProjectMapHitDto> {
    let mut hits = graph
        .nodes
        .iter()
        .filter(|n| n.complexity == "complex" || n.complexity == "moderate")
        .map(|n| hit_from_node(n, f32::from(complexity_rank(&n.complexity)) / 3.0))
        .collect::<Vec<_>>();
    hits.sort_by(|a, b| {
        complexity_rank(&b.complexity)
            .cmp(&complexity_rank(&a.complexity))
            .then_with(|| a.name.cmp(&b.name))
    });
    hits.truncate(limit);
    if hits.is_empty() {
        hits = graph
            .nodes
            .iter()
            .filter(|n| {
                is_file_level(&n.node_type)
                    || n.node_type == "module"
                    || n.node_type == "class"
                    || n.node_type == "function"
            })
            .map(|n| hit_from_node(n, 0.5))
            .collect::<Vec<_>>();
        hits.sort_by(|a, b| {
            hit_display_rank(a)
                .cmp(&hit_display_rank(b))
                .then_with(|| {
                    a.file_path
                        .as_deref()
                        .unwrap_or(&a.name)
                        .cmp(b.file_path.as_deref().unwrap_or(&b.name))
                })
                .then_with(|| a.name.cmp(&b.name))
        });
        hits.truncate(limit);
    }
    hits
}

fn graph_slice(graph: &RawGraph, limit: usize) -> ProjectMapGraphDto {
    let mut nodes = graph
        .nodes
        .iter()
        .map(|node| hit_from_node(node, graph_node_score(node)))
        .collect::<Vec<_>>();
    nodes.sort_by(|a, b| {
        graph_hit_rank(a)
            .cmp(&graph_hit_rank(b))
            .then_with(|| hit_display_rank(a).cmp(&hit_display_rank(b)))
            .then_with(|| {
                a.file_path
                    .as_deref()
                    .unwrap_or(&a.name)
                    .cmp(b.file_path.as_deref().unwrap_or(&b.name))
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    nodes.truncate(limit);
    let node_ids = nodes
        .iter()
        .map(|node| node.node_id.as_str())
        .collect::<HashSet<_>>();
    let mut edges = graph
        .edges
        .iter()
        .filter(|edge| {
            node_ids.contains(edge.source.as_str()) && node_ids.contains(edge.target.as_str())
        })
        .map(|edge| ProjectMapEdgeDto {
            source: edge.source.clone(),
            target: edge.target.clone(),
            edge_type: edge.edge_type.clone(),
            weight: edge._weight,
        })
        .collect::<Vec<_>>();
    edges.sort_by(|a, b| {
        edge_rank(&a.edge_type)
            .cmp(&edge_rank(&b.edge_type))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.target.cmp(&b.target))
    });
    edges.truncate(limit.saturating_mul(3));
    ProjectMapGraphDto { nodes, edges }
}

fn graph_node_score(node: &RawNode) -> f32 {
    let type_score = match node.node_type.as_str() {
        "class" => 0.95,
        "function" => 0.9,
        "endpoint" => 0.88,
        "service" => 0.82,
        "file" => 0.72,
        "module" => 0.5,
        _ => 0.6,
    };
    type_score + (f32::from(complexity_rank(&node.complexity)) * 0.02)
}

fn graph_hit_rank(hit: &ProjectMapHitDto) -> u8 {
    match hit.node_type.as_str() {
        "class" => 0,
        "function" => 1,
        "endpoint" => 2,
        "service" => 3,
        "file" => 4,
        "module" => 8,
        _ => 6,
    }
}

fn edge_rank(edge_type: &str) -> u8 {
    match edge_type {
        "calls" => 0,
        "imports" => 1,
        "contains" => 2,
        "routes" => 3,
        "related" => 6,
        _ => 5,
    }
}

fn hit_display_rank(hit: &ProjectMapHitDto) -> u8 {
    let path = hit.file_path.as_deref().unwrap_or("").to_lowercase();
    let type_rank: u8 = match hit.node_type.as_str() {
        "class" => 0,
        "function" => 1,
        "file" => 2,
        "module" => 4,
        _ => 3,
    };
    let path_rank: u8 =
        if path.starts_with("src/main/java/") || path.starts_with("src/main/kotlin/") {
            0
        } else if path.starts_with("src/") {
            1
        } else if path.starts_with('.') {
            9
        } else {
            3
        };
    path_rank.saturating_mul(10) + type_rank
}

fn node_map(graph: &RawGraph) -> HashMap<&str, &RawNode> {
    graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect()
}

fn resolve_node<'a>(graph: &'a RawGraph, node_ref: &str) -> Option<&'a RawNode> {
    graph.nodes.iter().find(|n| {
        n.id == node_ref
            || n.file_path.as_deref() == Some(node_ref)
            || n.id.strip_prefix("file:") == Some(node_ref)
    })
}

fn node_dto(node: &RawNode) -> ProjectMapNodeDto {
    ProjectMapNodeDto {
        id: node.id.clone(),
        node_type: node.node_type.clone(),
        name: node.name.clone(),
        file_path: node.file_path.clone(),
        line_range: node.line_range,
        summary: node.summary.clone(),
        tags: node.tags.clone(),
        complexity: node.complexity.clone(),
        language_notes: node.language_notes.clone(),
    }
}

fn hit_from_node(node: &RawNode, score: f32) -> ProjectMapHitDto {
    ProjectMapHitDto {
        node_id: node.id.clone(),
        node_type: node.node_type.clone(),
        name: node.name.clone(),
        file_path: node.file_path.clone(),
        summary: node.summary.chars().take(240).collect(),
        complexity: node.complexity.clone(),
        score,
    }
}

fn neighbor(edge_type: &str, direction: &str, node: &RawNode) -> ProjectMapNeighborDto {
    ProjectMapNeighborDto {
        edge_type: edge_type.to_string(),
        direction: direction.to_string(),
        node: hit_from_node(node, 1.0),
    }
}

fn neighbors_from_graph(graph: &RawGraph, node_id: &str) -> ProjectMapNeighborsDto {
    let by_id = node_map(graph);
    let node = by_id.get(node_id).map(|n| node_dto(n));
    let mut out = ProjectMapNeighborsDto {
        node,
        imports: Vec::new(),
        imported_by: Vec::new(),
        calls: Vec::new(),
        called_by: Vec::new(),
        related: Vec::new(),
    };
    for edge in &graph.edges {
        if edge.source == node_id {
            if let Some(target) = by_id.get(edge.target.as_str()) {
                match edge.edge_type.as_str() {
                    "imports" => out.imports.push(neighbor("imports", "out", target)),
                    "calls" => out.calls.push(neighbor("calls", "out", target)),
                    _ => out.related.push(neighbor(&edge.edge_type, "out", target)),
                }
            }
        }
        if edge.target == node_id {
            if let Some(source) = by_id.get(edge.source.as_str()) {
                match edge.edge_type.as_str() {
                    "imports" => out.imported_by.push(neighbor("imports", "in", source)),
                    "calls" => out.called_by.push(neighbor("calls", "in", source)),
                    _ => out.related.push(neighbor(&edge.edge_type, "in", source)),
                }
            }
        }
    }
    out.imports.truncate(50);
    out.imported_by.truncate(50);
    out.calls.truncate(50);
    out.called_by.truncate(50);
    out.related.truncate(50);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn refresh_deep_generates_graph_without_node_js() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write_file(
            root,
            "src/main.rs",
            "fn main() {\n    helper();\n}\n\nfn helper() {\n    println!(\"hi\");\n}\n",
        );
        write_file(
            root,
            "web/app.ts",
            "export function start(): void {\n    console.log('go');\n}\n",
        );

        let service = ProjectMapService::new();

        // First run: full index. Produces the UA knowledge-graph.json natively.
        let refresh = service.refresh_deep(root).unwrap();
        assert!(refresh.ok);
        assert!(refresh.nodes > 0, "graph should have nodes");
        assert!(
            Path::new(&refresh.graph_path).is_file(),
            "knowledge-graph.json must be written"
        );

        // The existing query API loads the freshly generated graph.
        let status = service.status(root);
        assert_eq!(status.status, "ready");
        assert!(status.nodes > 0);
        assert!(status.files >= 2);

        let overview = service.overview(root);
        assert!(overview.status.nodes > 0);

        // Second run: incremental sync over an unchanged tree still succeeds.
        let again = service.refresh_deep(root).unwrap();
        assert!(again.ok);
        assert!(again.nodes > 0);
    }

    #[test]
    fn complex_hits_falls_back_to_file_nodes() {
        let graph = RawGraph {
            project: None,
            nodes: vec![
                RawNode {
                    id: "file:src/app.tsx".to_string(),
                    node_type: "file".to_string(),
                    name: "app.tsx".to_string(),
                    file_path: Some("src/app.tsx".to_string()),
                    line_range: None,
                    summary: "App shell".to_string(),
                    tags: Vec::new(),
                    complexity: "simple".to_string(),
                    language_notes: None,
                },
                RawNode {
                    id: "file:src/api.ts".to_string(),
                    node_type: "file".to_string(),
                    name: "api.ts".to_string(),
                    file_path: Some("src/api.ts".to_string()),
                    line_range: None,
                    summary: "API client".to_string(),
                    tags: Vec::new(),
                    complexity: "simple".to_string(),
                    language_notes: None,
                },
            ],
            edges: Vec::new(),
        };

        let hits = complex_hits(&graph, 10);
        assert_eq!(hits.len(), 2);
        assert!(hits
            .iter()
            .any(|h| h.file_path.as_deref() == Some("src/app.tsx")));
        assert!(hits
            .iter()
            .any(|h| h.file_path.as_deref() == Some("src/api.ts")));
    }
}
