//! H-289 regression coverage through the persistent production facade.

use std::fs;
use std::path::Path;

use deepagent_codegraph::query::ExploreBudget;
use deepagent_codegraph::types::EdgeKind;
use deepagent_codegraph::CodeGraph;
use tempfile::TempDir;

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}
#[test]
fn audit_definition_change_reconnects_unchanged_caller() {
    let dir = TempDir::new().unwrap();
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() {\n    target();\n}\n",
    );
    write_file(dir.path(), "src/target.rs", "pub fn target() {}\n");

    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();
    let initial = graph.callers("target", 10).unwrap();
    assert_eq!(initial.len(), 1, "initial cross-file call must resolve");

    write_file(dir.path(), "src/target.rs", "pub fn renamed() {}\n");
    graph.sync().unwrap();
    assert!(
        graph.callers("target", 10).unwrap().is_empty(),
        "renaming the definition must remove the old edge"
    );

    // Restore only the definition. The caller source and its parked reference
    // are unchanged, so a correct dependency-aware sync should reconnect it.
    write_file(dir.path(), "src/target.rs", "pub fn target() {}\n");
    graph.sync().unwrap();
    let restored = graph.callers("target", 10).unwrap();

    // Touching the caller makes the current implementation re-run its parked
    // reference and demonstrates that the missing dependency invalidation is
    // specifically tied to definition-only changes.
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() {\n    target();\n}\n\n",
    );
    graph.sync().unwrap();
    let after_caller_touch = graph.callers("target", 10).unwrap();
    eprintln!(
        "incremental resolution audit: initial_callers={}, restored_callers={}, after_caller_touch={}",
        initial.len(),
        restored.len(),
        after_caller_touch.len()
    );
    assert_eq!(
        after_caller_touch.len(),
        1,
        "touching the caller is a control showing its parked reference can resolve"
    );
    assert_eq!(
        restored.len(),
        1,
        "restoring a changed definition must re-resolve unchanged callers"
    );
}

#[test]
fn definition_edit_preserves_local_calls_queries_projection_and_cold_reopen() {
    let dir = TempDir::new().unwrap();
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() { local(); target(); }\nfn local() {}\n",
    );
    write_file(dir.path(), "src/target.rs", "pub fn target() {}\n");
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();
    let original_caller = graph.node("caller").unwrap().unwrap().node;

    write_file(
        dir.path(),
        "src/target.rs",
        "\n\npub fn target(value: i32) -> i32 { value }\n",
    );
    assert_eq!(graph.sync().unwrap().files_indexed, 1);
    assert_eq!(graph.node("caller").unwrap().unwrap().node, original_caller);
    assert_complete_views(&graph);
    let edges = graph.store().all_edges().unwrap();
    for _ in 0..3 {
        assert_eq!(graph.sync().unwrap().files_indexed, 0);
        assert_eq!(graph.store().all_edges().unwrap(), edges);
    }
    drop(graph);
    let reopened = CodeGraph::open(dir.path()).unwrap();
    assert_complete_views(&reopened);
}

fn assert_complete_views(graph: &CodeGraph) {
    let caller = graph.node("caller").unwrap().unwrap().node;
    let target = graph.node("target").unwrap().unwrap().node;
    assert_eq!(graph.callers(&target.id, 10).unwrap()[0].node.id, caller.id);
    let callees = graph.callees(&caller.id, 10).unwrap();
    assert_eq!(callees.len(), 2, "keep the local AST call on the same line");
    assert!(callees.iter().any(|site| site.node.id == target.id));
    assert!(callees.iter().any(|site| site.node.name == "local"));
    assert_eq!(
        graph.impact(&target.id, 2).unwrap().unwrap().direct,
        vec![caller.clone()]
    );
    let flow = graph
        .explore(
            &["caller".into(), "target".into()],
            ExploreBudget::default(),
        )
        .unwrap();
    assert!(flow
        .flow
        .iter()
        .any(|hop| hop.from_node_id == caller.id && hop.to_node_id == target.id));

    let out = graph
        .project_root()
        .join(".understand-anything/knowledge-graph.json");
    graph.project_ua_json(&out).unwrap();
    let projected: serde_json::Value = serde_json::from_slice(&fs::read(out).unwrap()).unwrap();
    assert!(projected["edges"].as_array().unwrap().iter().any(|edge| {
        edge["source"] == caller.id && edge["target"] == target.id && edge["type"] == "related"
    }));
}

#[test]
fn added_and_deleted_candidates_replace_bindings_without_duplicate_edges() {
    let dir = TempDir::new().unwrap();
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() { target(); }\n",
    );
    write_file(dir.path(), "far/target.rs", "pub fn target() {}\n");
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();
    assert_eq!(
        graph.callees("caller", 10).unwrap()[0].node.file_path,
        "far/target.rs"
    );
    for _ in 0..2 {
        write_file(dir.path(), "src/target.rs", "pub fn target() {}\n");
        assert_eq!(graph.sync().unwrap().files_indexed, 1);
        let sites = graph.callees("caller", 10).unwrap();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].node.file_path, "src/target.rs");
        let far = graph
            .store()
            .all_nodes()
            .unwrap()
            .into_iter()
            .find(|node| node.name == "target" && node.file_path == "far/target.rs")
            .unwrap();
        assert!(
            graph.callers(&far.id, 10).unwrap().is_empty(),
            "remove the obsolete candidate binding"
        );
        fs::remove_file(dir.path().join("src/target.rs")).unwrap();
        assert_eq!(graph.sync().unwrap().files_indexed, 0);
        let sites = graph.callees("caller", 10).unwrap();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].node.file_path, "far/target.rs");
    }
}

#[test]
fn qualified_candidate_changes_reconnect_the_unchanged_reference() {
    let dir = TempDir::new().unwrap();
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() { Chosen::run(); }\n",
    );
    write_file(
        dir.path(),
        "src/other.rs",
        "struct Other;\nimpl Other { pub fn run() {} }\n",
    );
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();
    assert_eq!(graph.callees("caller", 10).unwrap().len(), 1);
    write_file(
        dir.path(),
        "src/chosen.rs",
        "struct Chosen;\nimpl Chosen { pub fn run() {} }\n",
    );
    graph.sync().unwrap();
    let sites = graph.callees("caller", 10).unwrap();
    assert_eq!(sites.len(), 1);
    assert!(sites[0].node.qualified_name.ends_with("Chosen::run"));
    write_file(
        dir.path(),
        "src/chosen.rs",
        "struct Changed;\nimpl Changed { pub fn run() {} }\n",
    );
    graph.sync().unwrap();
    write_file(
        dir.path(),
        "src/chosen.rs",
        "struct Chosen;\nimpl Chosen { pub fn run() {} }\n",
    );
    graph.sync().unwrap();
    let sites = graph.callees("caller", 10).unwrap();
    assert_eq!(sites.len(), 1);
    assert!(sites[0].node.qualified_name.ends_with("Chosen::run"));
}

#[test]
fn caller_and_target_in_the_same_batch_preserve_unrelated_calls() {
    let dir = TempDir::new().unwrap();
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() { target(); }\n",
    );
    write_file(dir.path(), "src/target.rs", "pub fn target() {}\n");
    write_file(
        dir.path(),
        "other.rs",
        "fn outer() { inner(); }\nfn inner() {}\n",
    );
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();
    let unrelated = graph.callees("outer", 10).unwrap();
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() { renamed(); renamed(); }\n",
    );
    write_file(dir.path(), "src/target.rs", "pub fn renamed() {}\n");
    assert_eq!(graph.sync().unwrap().files_indexed, 2);
    assert_eq!(graph.callees("caller", 10).unwrap().len(), 1);
    assert_eq!(graph.callees("caller", 10).unwrap()[0].node.name, "renamed");
    assert_eq!(graph.callees("outer", 10).unwrap(), unrelated);
}

#[test]
fn definition_only_updates_reconnect_typescript_python_and_go() {
    let fixtures = [
        (
            "ts",
            "import { target } from './target';\nexport function caller() { target(); }\n",
            "export function target() {}\n",
        ),
        (
            "py",
            "from target import target\ndef caller():\n    target()\n",
            "def target():\n    pass\n",
        ),
        (
            "go",
            "package main\nfunc caller() { target() }\n",
            "package main\nfunc target() {}\n",
        ),
    ];
    for (ext, caller_source, target_source) in fixtures {
        let dir = TempDir::new().unwrap();
        let caller_path = format!("src/caller.{ext}");
        let target_path = format!("src/target.{ext}");
        write_file(dir.path(), &caller_path, caller_source);
        write_file(dir.path(), &target_path, target_source);
        let mut graph = CodeGraph::open(dir.path()).unwrap();
        graph.index_all().unwrap();
        let initial_imports: Vec<_> = graph
            .store()
            .all_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.provenance.as_deref() == Some("import_resolver"))
            .collect();
        assert_eq!(
            graph.callers("target", 10).unwrap().len(),
            1,
            "{ext} initial"
        );
        fs::remove_file(dir.path().join(&target_path)).unwrap();
        graph.sync().unwrap();
        assert!(
            graph.callees("caller", 10).unwrap().is_empty(),
            "{ext} deleted"
        );
        drop(graph);
        write_file(dir.path(), &target_path, target_source);
        let mut graph = CodeGraph::open(dir.path()).unwrap();
        graph.sync().unwrap();
        assert_eq!(
            graph.callers("target", 10).unwrap().len(),
            1,
            "{ext} restored"
        );
        let restored_imports: Vec<_> = graph
            .store()
            .all_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.provenance.as_deref() == Some("import_resolver"))
            .collect();
        assert_eq!(
            restored_imports, initial_imports,
            "{ext} imports share the deleted file target"
        );
    }
}

#[test]
fn resolution_failure_rolls_back_the_entire_sync_and_can_be_retried() {
    assert_atomic_failure("CREATE TRIGGER h289_fail BEFORE INSERT ON edges WHEN NEW.kind='calls' BEGIN SELECT RAISE(ABORT, 'synthetic sync failure'); END;");
}

#[test]
fn metadata_failure_rolls_back_completed_resolution_and_can_be_retried() {
    assert_atomic_failure("CREATE TRIGGER h289_fail BEFORE UPDATE ON project_metadata WHEN NEW.key='last_changed_files' BEGIN SELECT RAISE(ABORT, 'synthetic sync failure'); END;");
}

fn assert_atomic_failure(trigger: &str) {
    let dir = TempDir::new().unwrap();
    write_file(
        dir.path(),
        "src/caller.rs",
        "pub fn caller() { target(); }\n",
    );
    write_file(dir.path(), "src/target.rs", "pub fn target() {}\n");
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    graph.index_all().unwrap();
    let nodes = graph.store().all_nodes().unwrap();
    let edges = graph.store().all_edges().unwrap();
    let refs = graph.store().all_unresolved_refs().unwrap();
    let metadata = graph.store().get_metadata("last_changed_files").unwrap();
    // Fault injection only; the production sync must still execute its real SQL.
    let faults =
        rusqlite::Connection::open(dir.path().join(".understand-anything/codegraph.db")).unwrap();
    faults.execute_batch(trigger).unwrap();
    write_file(dir.path(), "src/target.rs", "\n\npub fn target() {}\n");
    let error = graph
        .sync()
        .expect_err("resolution must run for the unchanged caller");
    assert!(
        error.to_string().contains("synthetic sync failure"),
        "{error}"
    );
    assert_eq!(graph.store().all_nodes().unwrap(), nodes);
    assert_eq!(graph.store().all_edges().unwrap(), edges);
    assert_eq!(graph.store().all_unresolved_refs().unwrap(), refs);
    assert_eq!(
        graph.store().get_metadata("last_changed_files").unwrap(),
        metadata
    );
    drop(graph);
    let mut graph = CodeGraph::open(dir.path()).unwrap();
    assert_eq!(graph.store().all_edges().unwrap(), edges);
    faults.execute_batch("DROP TRIGGER h289_fail;").unwrap();
    assert_eq!(
        graph.sync().unwrap().files_indexed,
        1,
        "failed sync must not advance the file hash"
    );
    assert_eq!(graph.callers("target", 10).unwrap().len(), 1);
    assert_eq!(
        graph
            .store()
            .all_edges()
            .unwrap()
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Calls)
            .count(),
        1
    );
}
