//! Resolve the fixture dependency before tests replace HOME/USERPROFILE.
//! A PATH entry may be a version-manager shim that needs the real user profile.
use std::path::PathBuf;
use std::time::Duration;

pub const NODE_RUNTIME_ENV: &str = "DEEPAGENT_MCP_FIXTURE_NODE";

pub async fn resolve_node_runtime() -> PathBuf {
    let mut command = tokio::process::Command::new("node");
    command
        .args(["--print", "process.execPath"])
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .expect("fixture Node runtime discovery exceeded five seconds")
        .expect("fixture Node runtime discovery must start");
    // Never print dependency stdout/stderr: a shim may include host configuration.
    assert!(
        output.status.success(),
        "fixture Node runtime discovery failed"
    );
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        panic!("fixture Node runtime path must be UTF-8");
    };
    let runtime = PathBuf::from(stdout.trim());
    assert!(
        runtime.is_absolute() && runtime.is_file(),
        "fixture Node runtime must be an existing absolute executable path"
    );
    runtime
}

pub fn isolated_node_runtime() -> PathBuf {
    let runtime = PathBuf::from(
        std::env::var_os(NODE_RUNTIME_ENV).expect("run the outer isolated environment audit"),
    );
    assert!(
        runtime.is_absolute() && runtime.is_file(),
        "isolated fixture Node runtime must be an existing absolute executable path"
    );
    runtime
}
