//! Opt-in audit probe for incremental call-edge invalidation.
//!
//! The production facade re-indexes changed files and re-resolves only call
//! references in those files. A definition change can therefore remove an
//! incoming edge without re-processing an unchanged caller.

use std::fs;
use std::path::Path;

use deepagent_codegraph::CodeGraph;
use tempfile::TempDir;

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}
#[test]
#[ignore = "audit reproducer: definition-only incremental changes must re-resolve unchanged callers"]
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
