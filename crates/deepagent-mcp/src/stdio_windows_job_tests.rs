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
    if matches!(
        mode.as_str(),
        "worker" | "worker-resource-heavy" | "worker-spawn-probe"
    ) {
        // Keep real file objects alive until process termination. This makes
        // kernel resource teardown observable without delaying the parent or
        // substituting a fake process handle/accounting result.
        let mut held_files = Vec::new();
        if mode == "worker-resource-heavy" {
            let resource = root.join("held-resource");
            std::fs::write(&resource, "SYNTHETIC_H46_RESOURCE").unwrap();
            for _ in 0..2_048 {
                held_files.push(std::fs::File::open(&resource).unwrap());
            }
        }
        let heartbeat = root.join("heartbeat");
        for _ in 0..400 {
            if mode == "worker-spawn-probe"
                && root.join("attempt-spawn").exists()
                && !root.join("spawn-result.json").exists()
            {
                let probe = root.join("probe");
                std::fs::create_dir_all(&probe).unwrap();
                let outcome = fixture_command("worker", &probe).spawn();
                let result = match outcome {
                    Ok(mut child) => {
                        child.kill().await.unwrap();
                        serde_json::json!({"started": true})
                    }
                    Err(error) => {
                        serde_json::json!({"started": false, "error": error.raw_os_error()})
                    }
                };
                std::fs::write(root.join("spawn-result.json"), result.to_string()).unwrap();
            }
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&heartbeat)
                .unwrap();
            file.write_all(b".").unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drop(held_files);
    } else if mode == "many" {
        let mut children = Vec::new();
        for index in 0..17 {
            let worker_root = root.join(index.to_string());
            std::fs::create_dir(&worker_root).unwrap();
            children.push(fixture_command("worker", &worker_root).spawn().unwrap());
            wait_file(&worker_root.join("heartbeat")).await;
        }
        let pids: Vec<_> = children.iter().map(|child| child.id().unwrap()).collect();
        std::fs::write(root.join("ready.json"), serde_json::to_vec(&pids).unwrap()).unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        drop(children);
    } else if matches!(
        mode.as_str(),
        "nested" | "nested-resource-heavy" | "nested-spawn-probe"
    ) {
        let worker_mode = match mode.as_str() {
            "nested" => "worker",
            "nested-spawn-probe" => "worker-spawn-probe",
            _ => "worker-resource-heavy",
        };
        let (child, _nested_job) =
            ProcessJob::spawn(&mut fixture_command(worker_mode, &root)).unwrap();
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
async fn shutdown_admission_barrier_blocks_nested_spawn_without_stopping_existing_members() {
    let root = tempfile::tempdir().unwrap();
    let (mut child, job) =
        ProcessJob::spawn(&mut fixture_command("nested-spawn-probe", root.path())).unwrap();
    let parent = observe_process(child.id().unwrap());
    let ready: serde_json::Value =
        serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
    let worker = observe_process(ready["worker"].as_u64().unwrap() as u32);
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe {
            QueryInformationJobObject(
                job.handle.as_raw_handle().cast(),
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of_mut!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
                std::ptr::null_mut(),
            )
        },
        0
    );
    let original_flags = limits.BasicLimitInformation.LimitFlags;
    let members = job.capture_shutdown_members().unwrap();
    assert_ne!(
        unsafe {
            QueryInformationJobObject(
                job.handle.as_raw_handle().cast(),
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of_mut!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
                std::ptr::null_mut(),
            )
        },
        0,
        "set admission barrier: {}",
        io::Error::last_os_error()
    );
    assert_eq!(
        limits.BasicLimitInformation.LimitFlags,
        original_flags | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
    );
    assert_eq!(limits.BasicLimitInformation.ActiveProcessLimit, 0);
    assert!(members.len() >= 2);
    assert!(is_running(&parent) && is_running(&worker));
    std::fs::write(root.path().join("attempt-spawn"), "SYNTHETIC_H46_BARRIER").unwrap();
    let result: serde_json::Value =
        serde_json::from_slice(&wait_file(&root.path().join("spawn-result.json")).await).unwrap();
    let existing_alive = is_running(&parent) && is_running(&worker);
    eprintln!("outer admission barrier: nested_spawn={result}, existing_alive={existing_alive}");
    job.terminate().unwrap();
    wait_exited(&parent).await;
    wait_exited(&worker).await;
    child.wait().await.unwrap();
    assert_eq!(result["started"], false);
    assert!(existing_alive);
    assert!(!root.path().join("probe/heartbeat").exists());
}

#[tokio::test]
async fn failed_assign_thread_discovery_and_resume_never_execute_or_leave_a_suspended_child() {
    for stage in ["assign", "snapshot", "owner", "resume"] {
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
                    return resume_child_threads(std::ptr::null_mut(), child.id().unwrap());
                }
                let process = child.raw_handle().unwrap().cast();
                if stage == "owner" {
                    return resume_child_threads(process, u32::MAX);
                }
                resume_child_threads_with(process, child.id().unwrap(), |_| unsafe {
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
            job.shutdown().await.unwrap();
            eprintln!(
                "nested after owned member shutdown: root_running={}, worker_running={}",
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
    job.shutdown().await.unwrap();
}

#[tokio::test]
async fn stdio_close_observes_nested_process_exit_at_the_public_return_boundary() {
    check_stdio_close_return_boundary("nested", 5).await;
}

#[tokio::test]
async fn stdio_close_waits_for_descendant_resource_teardown() {
    check_stdio_close_return_boundary("nested-resource-heavy", 1).await;
}

#[tokio::test]
async fn terminating_member_remains_capturable_before_process_signal() {
    use windows_sys::Win32::System::Threading::{TerminateProcess, PROCESS_TERMINATE};

    let root = tempfile::tempdir().unwrap();
    let (mut child, job) =
        ProcessJob::spawn(&mut fixture_command("nested-resource-heavy", root.path())).unwrap();
    let ready: serde_json::Value =
        serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
    let pid = ready["worker"].as_u64().unwrap() as u32;
    let worker = observe_process(pid);
    let kill = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    assert!(!kill.is_null());
    let kill = unsafe { OwnedHandle::from_raw_handle(kill.cast()) };
    assert_ne!(
        unsafe { TerminateProcess(kill.as_raw_handle().cast(), 1) },
        0
    );
    let ids = job.member_ids().unwrap();
    let worker_running = is_running(&worker);
    eprintln!("independently terminating member: ids={ids:?}, worker_running={worker_running}");
    job.shutdown().await.unwrap();
    child.wait().await.unwrap();
    wait_exited(&worker).await;
    assert!(
        !worker_running || ids.contains(&pid),
        "an unsignaled member cannot be lost before capture"
    );
}

#[tokio::test]
async fn cancelled_and_concurrent_public_close_retain_the_original_member_handles() {
    use crate::McpTransport;
    use std::future::Future;

    let root = tempfile::tempdir().unwrap();
    let transport = fixture_transport("nested-resource-heavy", root.path());
    let ready: serde_json::Value =
        serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
    let parent = observe_process(ready["root"].as_u64().unwrap() as u32);
    let worker = observe_process(ready["worker"].as_u64().unwrap() as u32);
    // The real root Child lock guarantees a cancellable boundary even on a
    // machine where every member finishes exiting during the first poll.
    let child_guard = transport.child.lock().await;
    let mut close = Box::pin(transport.close());
    let first_poll =
        std::future::poll_fn(|cx| std::task::Poll::Ready(close.as_mut().poll(cx))).await;
    assert!(first_poll.is_pending());
    let retained: Vec<_> = transport
        .process_job
        .shutdown_members
        .get()
        .unwrap()
        .as_ref()
        .unwrap()
        .iter()
        .map(|handle| handle.as_raw_handle() as usize)
        .collect();
    assert!(retained.len() >= 2);
    drop(close);
    drop(child_guard);
    let (first, second) = tokio::join!(transport.close(), transport.close());
    first.unwrap();
    second.unwrap();
    let retried: Vec<_> = transport
        .process_job
        .shutdown_members
        .get()
        .unwrap()
        .as_ref()
        .unwrap()
        .iter()
        .map(|handle| handle.as_raw_handle() as usize)
        .collect();
    assert_eq!(
        retained, retried,
        "retry must not replace the original exit evidence"
    );
    assert!(!is_running(&parent) && !is_running(&worker));
    eprintln!(
        "cancelled public close: retained_members={}, concurrent_retry=ok",
        retained.len()
    );
}

#[tokio::test]
async fn capture_failure_terminates_members_and_cannot_become_success_on_retry() {
    let root = tempfile::tempdir().unwrap();
    let (mut child, job) = ProcessJob::spawn(&mut fixture_command("nested", root.path())).unwrap();
    let ready: serde_json::Value =
        serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
    let parent = observe_process(ready["root"].as_u64().unwrap() as u32);
    let worker = observe_process(ready["worker"].as_u64().unwrap() as u32);
    // Inject a real OpenProcess failure at capture, after the actual admission
    // barrier. Do not substitute fake handles or a successful termination stub.
    let error = job
        .begin_shutdown_with(|job| {
            let members = job.capture_shutdown_members()?;
            job.open_member(0)?;
            Ok(members)
        })
        .as_ref()
        .unwrap_err()
        .to_string();
    wait_exited(&parent).await;
    wait_exited(&worker).await;
    child.wait().await.unwrap();
    assert!(error.contains("open MCP job member 0"));
    for _ in 0..2 {
        let retry = job.shutdown().await.unwrap_err();
        assert!(retry.to_string().contains(&error));
    }
    eprintln!("capture error retained after actual job termination: {error}");
}

#[tokio::test]
async fn member_capture_rejects_a_handle_outside_the_owned_job() {
    let root = tempfile::tempdir().unwrap();
    let (mut child, job) = ProcessJob::spawn(&mut fixture_command("worker", root.path())).unwrap();
    wait_file(&root.path().join("heartbeat")).await;
    let parent = observe_process(child.id().unwrap());
    let error = job.open_member(std::process::id()).unwrap_err();
    assert!(error.to_string().contains("identity changed"));
    assert!(
        is_running(&parent),
        "validation must not terminate a process"
    );
    job.shutdown().await.unwrap();
    child.wait().await.unwrap();
    assert!(!is_running(&parent));
}

#[tokio::test]
async fn member_enumeration_grows_and_waits_for_every_real_child() {
    let root = tempfile::tempdir().unwrap();
    let (mut child, job) = ProcessJob::spawn(&mut fixture_command("many", root.path())).unwrap();
    let pids: Vec<u32> =
        serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
    let workers: Vec<_> = pids.iter().map(|&pid| observe_process(pid)).collect();
    let members = job.member_ids().unwrap();
    assert!(
        members.len() > 16,
        "exercise the actual variable-size API buffer"
    );
    assert!(pids.iter().all(|pid| members.contains(pid)));
    job.shutdown().await.unwrap();
    assert!(workers.iter().all(|worker| !is_running(worker)));
    child.wait().await.unwrap();
    eprintln!(
        "dynamic member enumeration: children={}, members={}",
        pids.len(),
        members.len()
    );
}

#[tokio::test]
async fn pending_member_wait_times_out_without_forgetting_handles_and_can_be_retried() {
    let root = tempfile::tempdir().unwrap();
    let (mut child, job) = ProcessJob::spawn(&mut fixture_command("worker", root.path())).unwrap();
    wait_file(&root.path().join("heartbeat")).await;
    let parent = observe_process(child.id().unwrap());
    // Unit-test the waiting phase with a genuinely pending owned process: seed
    // the captured state but deliberately withhold the termination request.
    // This is not evidence of a real TerminateJobObject timeout.
    job.shutdown_members
        .set(Ok(job.capture_shutdown_members().unwrap()))
        .unwrap();
    let error = job.shutdown().await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(is_running(&parent));
    let retained = job.shutdown_members.get().unwrap().as_ref().unwrap().len();
    job.terminate().unwrap();
    job.shutdown().await.unwrap();
    child.wait().await.unwrap();
    assert!(!is_running(&parent));
    assert_eq!(
        job.shutdown_members.get().unwrap().as_ref().unwrap().len(),
        retained
    );
    eprintln!("pending member wait: timeout={error}, retained_members={retained}, retry=ok");
}

fn fixture_transport(mode: &str, root: &Path) -> crate::StdioTransport {
    let config = crate::McpServerConfig {
        response_limit_bytes: None,
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
            ("DEEPAGENT_H46_FIXTURE_MODE".into(), mode.into()),
            (
                "DEEPAGENT_H46_FIXTURE_ROOT".into(),
                root.to_string_lossy().into_owned(),
            ),
        ]
        .into(),
        cwd: None,
        url: None,
        headers: Default::default(),
    };
    crate::StdioTransport::spawn(&config).unwrap()
}

async fn check_stdio_close_return_boundary(mode: &str, rounds: usize) {
    use crate::McpTransport;

    for round in 0..rounds {
        let root = tempfile::tempdir().unwrap();
        let transport = fixture_transport(mode, root.path());
        let ready: serde_json::Value =
            serde_json::from_slice(&wait_file(&root.path().join("ready.json")).await).unwrap();
        let parent = observe_process(ready["root"].as_u64().unwrap() as u32);
        let worker = observe_process(ready["worker"].as_u64().unwrap() as u32);
        assert!(is_running(&parent) && is_running(&worker));
        transport.close().await.unwrap();
        let parent_running = is_running(&parent);
        let worker_running = is_running(&worker);
        eprintln!("public close mode={mode} round={round}: root_running={parent_running}, worker_running={worker_running}");
        // Preserve the immediate return-boundary observation, but finish this
        // fixture's own cleanup before asserting a known failure.
        wait_exited(&parent).await;
        wait_exited(&worker).await;
        assert!(
            !parent_running && !worker_running,
            "successful public close must observe exit: mode={mode}, root_running={parent_running}, worker_running={worker_running}"
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
