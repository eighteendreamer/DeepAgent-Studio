//! Opt-in real-child checks. All source/data are fixed synthetic fixtures;
//! they use the production path, not a host Node/shell or a mock receipt.
use super::*;
use deepagent_runtime::workflow::code::CodeOutput;
use std::sync::mpsc;
use std::time::Duration;

struct Observed {
    process: OwnedHandle,
    invocation: PathBuf,
    profile_folder: PathBuf,
}
impl Observed {
    fn resumed(child: &Child, stage: &Path, profile: &Profile) -> Self {
        process::assert_child_job_limits(child);
        let observed = Self {
            process: child.process.try_clone().unwrap(),
            invocation: stage.parent().unwrap().to_path_buf(),
            profile_folder: PathBuf::from(profile.folder().unwrap()),
        };
        assert!(stage.is_dir() && observed.profile_folder.is_dir());
        observed
    }

    fn assert_cleaned(&self) {
        assert_eq!(
            unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert!(
            !self.invocation.exists(),
            "invocation staging must be removed"
        );
        assert!(
            !self.profile_folder.exists(),
            "invocation profile folder must be removed"
        );
    }
}

fn numeric_request(script: &str) -> (tempfile::TempDir, SandboxCodeRequest) {
    let (root, mut request) = sdk_request();
    request.code.script = script.into();
    request.code.outputs = vec![CodeOutput {
        name: "result".into(),
        value_type: "number".into(),
    }];
    request.context = request.context.with_timeout(Duration::from_secs(15));
    (root, request)
}

fn checked_execution(request: SandboxCodeRequest) -> Result<CodeResponse> {
    let mut observed = None;
    let result = execute_observed(
        request,
        Arc::new(AtomicBool::new(false)),
        |child, stage, profile| {
            observed = Some(Observed::resumed(child, stage, profile));
        },
    );
    observed
        .expect("production child must really resume before result validation")
        .assert_cleaned();
    result
}

#[test]
#[ignore = "requires opt-in managed Node and unpackaged terminal; real errors/log/frame validation"]
fn h178_production_node_validates_errors_outputs_and_bounded_logs() {
    for (script, error_kind) in [
        ("function main( { H178_PRIVATE_SOURCE", "script syntax error"),
        ("function main() { throw new Error('H178_PRIVATE_INPUT'); }", "script runtime error"),
        ("function main() { return {}; }", "CodeResultInvalid"),
        ("function main() { return {result:'42'}; }", "CodeResultInvalid"),
        ("function main() { return {result:42,extra:1}; }", "CodeResultInvalid"),
        ("function main() { return null; }", "script runtime error"),
        ("function main() { return {result:'x'.repeat(2*1024*1024)}; }", "script runtime error"),
        ("function main() { require('node:fs').writeSync(1, Buffer.alloc(2*1024*1024,120)); return {result:42}; }", "CodeResultInvalid"),
    ] {
        let (_root, request) = numeric_request(script);
        let error = checked_execution(request).err().expect("no fabricated success").to_string();
        assert!(error.contains(error_kind), "expected {error_kind}, got {error}");
        assert!(!error.contains("H178_PRIVATE_SOURCE") && !error.contains("H178_PRIVATE_INPUT"));
    }
    let (_root, request) = numeric_request("function main() { process.stdout.write('界'.repeat(20000)); process.stderr.write('界'.repeat(20000)); return {result:42}; }");
    let response = checked_execution(request).expect("bounded logs must preserve real result");
    assert_eq!(response.outputs["result"], 42);
    assert!(response.receipt.stdout_truncated && response.receipt.stderr_truncated);
    assert!(!response.receipt.stdout.is_empty() && !response.receipt.stderr.is_empty());
    assert!(response.receipt.stdout.len() <= MAX_CODE_LOG_BYTES);
    assert!(response.receipt.stderr.len() <= MAX_CODE_LOG_BYTES);
    println!("production_errors_outputs_bounded_logs_and_cleanup_passed=true");
}

#[test]
#[ignore = "requires opt-in managed Node and unpackaged terminal; real resource/environment controls"]
fn h178_production_node_denies_child_and_excess_memory_without_ambient_env() {
    let (_root, mut request) = numeric_request(
        r#"function main() {
        // Inherit only the existing stdio for this fixed, must-not-start child.
        // Default stdio creates named pipes before uv_spawn reaches process
        // creation; LPAC denial can stall libuv's pipe-name collision retry.
        const child = require('node:child_process').spawnSync(process.execPath,
            ['--no-addons','-e','process.exit(0)'], {timeout:2000,windowsHide:true,stdio:'inherit'});
        return { result: child.error && !child.pid && child.status === null ? 42 : 0,
            spawnError: child.error ? String(child.error.code) : 'NONE',
            env: Object.keys(process.env).sort() };
    }"#,
    );
    request.code.outputs.push(CodeOutput {
        name: "env".into(),
        value_type: "array".into(),
    });
    request.code.outputs.push(CodeOutput {
        name: "spawnError".into(),
        value_type: "string".into(),
    });
    let response = checked_execution(request).expect("the parent must survive denied spawn");
    assert_eq!(
        response.outputs["result"], 42,
        "no second process may start"
    );
    let spawn_error = response.outputs["spawnError"].as_str().unwrap();
    assert!(!matches!(spawn_error, "ETIMEDOUT" | "ENOENT" | "NONE"));
    println!("production_child_spawn_denied_error={spawn_error} pid_absent=true");
    let mut keys: Vec<_> = response.outputs["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap().to_ascii_uppercase())
        .collect();
    keys.sort();
    assert_eq!(keys, ["LOCALAPPDATA", "SYSTEMROOT", "TEMP", "TMP"]);
    let (_root, request) = numeric_request("function main() { const tooLarge = Buffer.alloc(300*1024*1024, 1); return {result:tooLarge.length}; }");
    let error = checked_execution(request)
        .err()
        .expect("300MiB allocation must not succeed under the actual 256MiB job cap")
        .to_string();
    assert!(
        error.contains("CodeExecutionFailed"),
        "unexpected memory outcome: {error}"
    );
    println!("production_job_membership_single_process_memory_env_and_cleanup_passed=true");
}

#[test]
#[ignore = "requires opt-in managed Node and unpackaged terminal; real timeout/token/flag/abandon cleanup"]
fn h178_production_node_timeout_and_cancellation_reap_the_actual_child() {
    for mode in ["timeout", "token", "flag", "abandoned"] {
        let (_root, mut request) = numeric_request("function main() { while (true) {} }");
        request.context = request
            .context
            .with_timeout(Duration::from_secs(if mode == "timeout" { 2 } else { 15 }));
        let context = request.context.clone();
        let abandoned = Arc::new(AtomicBool::new(false));
        let signal = abandoned.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let started = Instant::now();
        let worker = std::thread::spawn(move || {
            execute_observed(request, abandoned, |child, stage, profile| {
                sender
                    .send(Observed::resumed(child, stage, profile))
                    .unwrap();
            })
        });
        let observed = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("real child must resume");
        // Do not accept a startup failure or pre-launch cancellation as evidence.
        assert_eq!(
            unsafe { WaitForSingleObject(observed.process.as_raw_handle(), 100) },
            WAIT_TIMEOUT
        );
        match mode {
            "token" => context.cancellation_token().cancel(),
            "flag" => context.cancel_flag().store(true, Ordering::Release),
            "abandoned" => signal.store(true, Ordering::Release),
            _ => {}
        }
        let error = worker
            .join()
            .unwrap()
            .err()
            .expect("infinite script cannot succeed")
            .to_string();
        assert!(
            error.contains(if mode == "timeout" {
                "CodeExecutionTimeout"
            } else {
                "CodeExecutionCancelled"
            }),
            "{mode}: {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "bounded reap for {mode}"
        );
        observed.assert_cleaned();
        println!("production_control={mode} actual_child_terminated_staging_profile_removed=true");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires opt-in managed Node and unpackaged terminal; drops async owner of a real blocking SDK child"]
async fn h178_production_node_dropped_future_reaps_the_actual_child() {
    let (_root, request) = numeric_request("function main() { while (true) {} }");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let owner = tokio::spawn(super::super::super::run_blocking(move |abandoned| {
        let result = execute_observed(request, abandoned, |child, stage, profile| {
            assert!(started_tx
                .send(Observed::resumed(child, stage, profile))
                .is_ok());
        });
        let _ = finished_tx.send(result.as_ref().err().map(ToString::to_string));
        result
    }));
    let observed = tokio::time::timeout(Duration::from_secs(10), started_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(observed.process.as_raw_handle(), 100) },
        WAIT_TIMEOUT
    );
    owner.abort();
    assert!(matches!(owner.await, Err(error) if error.is_cancelled()));
    let error = tokio::time::timeout(Duration::from_secs(5), finished_rx)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(error.contains("CodeExecutionCancelled"));
    observed.assert_cleaned();
    println!("production_dropped_async_owner_actual_child_terminated_staging_profile_removed=true");
}
