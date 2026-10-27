//! Native lifecycle fixtures execute this test binary, never a user program.
use super::*;

use std::path::Path;
use std::process::Stdio;

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB, PROCESS_SYNCHRONIZE,
};

const CHILD_ENTRY: &str = "stdio::windows_job::tests::native_process_fixture";

fn fixture_command(mode: &str, root: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD_ENTRY, "--ignored", "--nocapture"])
        .env("DEEPAGENT_H46_FIXTURE_MODE", mode)
        .env("DEEPAGENT_H46_FIXTURE_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true);
    command
}

fn observe_process(pid: u32) -> OwnedHandle {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(
        !handle.is_null(),
        "open native fixture: {}",
        io::Error::last_os_error()
    );
    // SAFETY: OpenProcess returned a valid owned handle to this fixture.
    unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
}

fn is_running(handle: &OwnedHandle) -> bool {
    let state = unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 0) };
    assert!(matches!(state, WAIT_OBJECT_0 | WAIT_TIMEOUT));
    state == WAIT_TIMEOUT
}

async fn wait_exited(handle: &OwnedHandle) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while is_running(handle) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned native fixture must exit, not remain suspended");
}

async fn wait_file(path: &Path) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = std::fs::read(path) {
                if !bytes.is_empty() {
                    return bytes;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native fixture must report actual execution")
}

#[tokio::test]
#[ignore = "subprocess entry; invoked only by the native process ownership tests"]
async fn native_process_fixture() {
    let mode = std::env::var("DEEPAGENT_H46_FIXTURE_MODE").unwrap();
    let root = std::path::PathBuf::from(std::env::var_os("DEEPAGENT_H46_FIXTURE_ROOT").unwrap());
    if mode == "worker" {
        let heartbeat = root.join("heartbeat");
        for _ in 0..400 {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&heartbeat)
                .unwrap();
            file.write_all(b".").unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    } else if mode == "nested" {
        let (child, _nested_job) =
            ProcessJob::spawn(&mut fixture_command("worker", &root)).unwrap();
        let pid = child.id().unwrap();
        wait_file(&root.join("heartbeat")).await;
        std::fs::write(
            root.join("ready.json"),
            serde_json::json!({"root": std::process::id(), "worker": pid}).to_string(),
        )
        .unwrap();
        // Keep both handles alive until the outer owner stops us. A finite
        // watchdog bounds unexpected fixture failure without external cleanup.
        tokio::time::sleep(Duration::from_secs(8)).await;
        drop(child);
    } else if mode == "breakaway" {
        let outcome = fixture_command("worker", &root)
            .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
            .spawn();
        let result = match outcome {
            Ok(mut child) => {
                child.kill().await.unwrap();
                serde_json::json!({"started": true})
            }
            Err(error) => serde_json::json!({"started": false, "error": error.raw_os_error()}),
        };
        std::fs::write(root.join("ready.json"), result.to_string()).unwrap();
    } else {
        panic!("unknown native fixture mode");
    }
}

#[tokio::test]
async fn failed_assign_thread_discovery_and_resume_never_execute_or_leave_a_suspended_child() {
    for stage in ["assign", "snapshot", "resume"] {
        let root = tempfile::tempdir().unwrap();
        let mut observed = None;
        let result = ProcessJob::spawn_with_setup(
            &mut fixture_command("worker", root.path()),
            |job, child| {
                observed = Some(observe_process(child.id().unwrap()));
                if stage == "assign" {
                    // Exercise a real AssignProcessToJobObject failure before any
                    // resume, with the exact same error propagation and owner drop.
                    return job.assign_process(std::ptr::null_mut());
                }
                job.assign_process(child.raw_handle().unwrap().cast())?;
                if stage == "snapshot" {
                    // ToolHelp succeeds, but the target has no thread in it.
                    return resume_child_threads(u32::MAX);
                }
                resume_child_threads_with(child.id().unwrap(), |_| unsafe {
                    ResumeThread(std::ptr::null_mut())
                })
            },
        );
        let error = match result {
            Ok(_) => panic!("injected {stage} failure must fail closed"),
            Err(error) => error,
        };
        eprintln!("startup stage={stage}, returned_error={error}");
        wait_exited(observed.as_ref().unwrap()).await;
        assert!(
            !root.path().join("heartbeat").exists(),
            "{stage}: child code ran"
        );
        assert!(!root.path().join("ready.json").exists());
    }
}

#[tokio::test]
async fn outer_job_shutdown_covers_a_real_nested_job() {
    for explicit_terminate in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let (mut child, job) =
            ProcessJob::spawn(&mut fixture_command("nested", root.path())).unwrap();
        let parent = observe_process(child.id().unwrap());
        let ready: serde_json::Value =
            serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
        let worker = observe_process(ready["worker"].as_u64().unwrap() as u32);
        assert!(is_running(&parent) && is_running(&worker));
        if explicit_terminate {
            job.terminate().unwrap();
            job.wait_empty().await.unwrap();
            eprintln!(
                "nested after accounting zero: root_running={}, worker_running={}",
                is_running(&parent),
                is_running(&worker)
            );
            child.wait().await.unwrap(); // same final root wait as StdioTransport::close
            eprintln!(
                "nested after root wait: root_running={}, worker_running={}",
                is_running(&parent),
                is_running(&worker)
            );
            assert!(
                !is_running(&parent) && !is_running(&worker),
                "close must observe nested exit"
            );
        }
        drop(job);
        wait_exited(&parent).await;
        wait_exited(&worker).await;
        child.wait().await.unwrap();
        let before = std::fs::metadata(root.path().join("heartbeat"))
            .unwrap()
            .len();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::metadata(root.path().join("heartbeat"))
                .unwrap()
                .len(),
            before
        );
    }
}

#[tokio::test]
async fn service_cannot_request_breakaway_from_its_owner_job() {
    let root = tempfile::tempdir().unwrap();
    let (mut child, job) =
        ProcessJob::spawn(&mut fixture_command("breakaway", root.path())).unwrap();
    let result: serde_json::Value =
        serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
    assert!(child.wait().await.unwrap().success());
    assert_eq!(result["started"], false);
    assert_eq!(
        result["error"], 5,
        "CreateProcess must report access denied"
    );
    assert!(!root.path().join("heartbeat").exists());
    job.wait_empty().await.unwrap();
}

#[tokio::test]
async fn stdio_close_observes_nested_process_exit_at_the_public_return_boundary() {
    use crate::{McpServerConfig, McpTransport, StdioTransport};

    for _ in 0..5 {
        let root = tempfile::tempdir().unwrap();
        let config = McpServerConfig {
            transport: None,
            command: Some(
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            args: vec![
                "--exact".into(),
                CHILD_ENTRY.into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            env: [
                ("DEEPAGENT_H46_FIXTURE_MODE".into(), "nested".into()),
                (
                    "DEEPAGENT_H46_FIXTURE_ROOT".into(),
                    root.path().to_string_lossy().into_owned(),
                ),
            ]
            .into(),
            cwd: None,
            url: None,
            headers: Default::default(),
        };
        let transport = StdioTransport::spawn(&config).unwrap();
        let ready: serde_json::Value =
            serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
        let parent = observe_process(ready["root"].as_u64().unwrap() as u32);
        let worker = observe_process(ready["worker"].as_u64().unwrap() as u32);
        assert!(is_running(&parent) && is_running(&worker));
        transport.close().await.unwrap();
        assert!(
            !is_running(&parent) && !is_running(&worker),
            "successful public close must observe exit"
        );
        transport.close().await.unwrap();
        let before = std::fs::metadata(root.path().join("heartbeat"))
            .unwrap()
            .len();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::metadata(root.path().join("heartbeat"))
                .unwrap()
                .len(),
            before
        );
    }
}
