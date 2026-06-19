//! Integration tests for the [`CodeGraph`] facade: full index, UA projection,
//! and a no-change incremental sync over a small multi-language project.
//!
//! These exercise the whole phase-one pipeline end to end (scan -> extract ->
//! store -> resolve imports -> project) with no external process dependency.

use std::fs;
use std::path::Path;
use std::time::Duration;

use deepagent_codegraph::types::NodeKind;
use deepagent_codegraph::CodeGraph;
use serde_json::Value;
use tempfile::TempDir;

/// Materialise `rel` under `root` with `contents`, creating parent dirs.
fn write_file(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// Build a tiny project spanning Rust, TypeScript, and Python (plus a
/// non-source file) so extraction across grammars and `Other` registration are
/// all covered.
fn sample_project() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write_file(
        root,
        "src/main.rs",
        "mod util;\n\nfn main() {\n    let n = util::add(1, 2);\n    println!(\"{n}\");\n}\n",
    );
    write_file(
        root,
        "src/util.rs",
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct Calc {\n    pub total: i32,\n}\n",
    );
    write_file(
        root,
        "web/app.ts",
        "import { greet } from './lib';\n\nexport function start(): void {\n    greet('world');\n}\n",
    );
    write_file(
        root,
        "web/lib.ts",
        "export function greet(name: string): string {\n    return `hi ${name}`;\n}\n",
    );
    write_file(
        root,
        "scripts/run.py",
        "def main():\n    total = compute(3)\n    print(total)\n\n\ndef compute(x):\n    return x * 2\n",
    );
    write_file(
        root,
        "README.md",
        "# Sample\n\nA tiny multi-language project.\n",
    );

    dir
}

#[test]
fn index_all_builds_graph_with_file_nodes_and_edges() {
    let dir = sample_project();
    let mut graph = CodeGraph::open(dir.path()).unwrap();

    assert!(
        !graph.has_existing_index(),
        "fresh database should report no index"
    );

    let stats = graph.index_all().unwrap();

    assert!(!stats.is_incremental, "index_all must be a full run");
    assert!(stats.files_indexed >= 6, "all sample files scanned");
    assert!(stats.nodes > 0, "graph should contain nodes");
    assert!(stats.edges > 0, "graph should contain edges");

    // File nodes for every source file plus the README.
    let file_nodes = graph.store().all_file_nodes().unwrap();
    let file_paths: Vec<String> = file_nodes.iter().map(|n| n.file_path.clone()).collect();
    for expected in [
        "src/main.rs",
        "src/util.rs",
        "web/app.ts",
        "web/lib.ts",
        "scripts/run.py",
        "README.md",
    ] {
        assert!(
            file_paths.iter().any(|p| p == expected),
            "missing file node for {expected}; got {file_paths:?}"
        );
    }

    // Symbol extraction produced non-file nodes too (functions/structs/...).
    let all_nodes = graph.store().all_nodes().unwrap();
    assert!(
        all_nodes.iter().any(|n| n.kind != NodeKind::File),
        "expected symbol nodes beyond file nodes"
    );

    // After indexing, an index exists.
    assert!(graph.has_existing_index());
}

#[test]
fn project_ua_json_matches_front_end_schema() {
    let dir = sample_project();
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();

    let out = dir.path().join(".understand-anything/knowledge-graph.json");
    let stats = graph.project_ua_json(&out).unwrap();
    assert!(stats.nodes > 0);

    let raw = fs::read_to_string(&out).unwrap();
    let json: Value = serde_json::from_str(&raw).unwrap();

    // Top-level fields the front-end panel expects.
    assert_eq!(json["version"], "1.0.0");
    assert!(json["project"].is_object(), "project metadata present");
    assert!(json["nodes"].is_array(), "nodes array present");
    assert!(json["edges"].is_array(), "edges array present");
    assert!(json["layers"].is_array(), "layers array present");
    assert!(json["tour"].is_array(), "tour array present");

    // Project metadata block carries the documented keys.
    let project = &json["project"];
    assert!(project["name"].is_string());
    assert!(project["languages"].is_array());
    assert!(project["frameworks"].is_array());

    // Every edge references node ids that exist in the node set.
    let node_ids: std::collections::BTreeSet<String> = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    for edge in json["edges"].as_array().unwrap() {
        let source = edge["source"].as_str().unwrap();
        let target = edge["target"].as_str().unwrap();
        assert!(node_ids.contains(source), "edge source {source} missing");
        assert!(node_ids.contains(target), "edge target {target} missing");
    }
}

#[test]
fn sync_with_no_changes_is_incremental_and_preserves_graph() {
    let dir = sample_project();
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    let full = graph.index_all().unwrap();

    // A second open reuses the existing database.
    let mut reopened = CodeGraph::open(dir.path()).unwrap();
    assert!(reopened.has_existing_index());

    let incremental = reopened.sync().unwrap();
    assert!(incremental.is_incremental, "sync must be incremental");
    assert_eq!(
        incremental.files_indexed, 0,
        "no files changed, so none re-indexed"
    );
    // Graph totals are unchanged by a no-op sync.
    assert_eq!(incremental.nodes, full.nodes);
    assert_eq!(incremental.edges, full.edges);
}

#[test]
fn sync_after_modification_reindexes_only_changed_file() {
    let dir = sample_project();
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();

    // Modify one file's contents (changes its content hash).
    write_file(
        dir.path(),
        "scripts/run.py",
        "def main():\n    print('changed')\n\n\ndef helper():\n    return 42\n\n\ndef extra():\n    return 7\n",
    );

    let stats = graph.sync().unwrap();
    assert!(stats.is_incremental);
    assert_eq!(
        stats.files_indexed, 1,
        "only the modified file should be re-indexed"
    );

    // The new symbol is present; the graph still has the other files' nodes.
    let nodes = graph.store().all_nodes().unwrap();
    assert!(
        nodes.iter().any(|n| n.name == "helper"),
        "re-extracted symbol should be present"
    );
    assert!(
        nodes.iter().any(|n| n.file_path == "src/main.rs"),
        "untouched files should remain in the graph"
    );
}

#[test]
fn indexes_1000_file_project_under_30_seconds_and_sync_is_smaller() {
    let dir = TempDir::new().unwrap();
    for i in 0..1000 {
        write_file(
            dir.path(),
            &format!("src/mod_{i}.rs"),
            &format!("pub fn f_{i}() -> usize {{ {i} }}\n"),
        );
    }

    let mut graph = CodeGraph::open(dir.path()).unwrap();
    let full = graph.index_all().unwrap();

    assert_eq!(full.files_indexed, 1000);
    assert!(
        full.duration < Duration::from_secs(30),
        "1000-file full index should stay under 30s, got {:?}",
        full.duration
    );

    write_file(
        dir.path(),
        "src/mod_42.rs",
        "pub fn f_42() -> usize { 4242 }\npub fn extra() -> usize { 7 }\n",
    );
    let sync = graph.sync().unwrap();

    assert!(sync.is_incremental);
    assert_eq!(sync.files_indexed, 1);
    assert!(
        sync.duration < full.duration,
        "single-file sync should be faster than full index ({:?} vs {:?})",
        sync.duration,
        full.duration
    );
}
