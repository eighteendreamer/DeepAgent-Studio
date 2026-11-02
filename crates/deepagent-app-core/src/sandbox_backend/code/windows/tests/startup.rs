//! Opt-in native startup diagnostics, never a production stderr disclosure path.
//! Both fixtures are fixed here: no workflow source, user inputs or credentials.
//! Fixed fixtures share the production launcher, limits, pipes and AAP gates.
use super::*;
use deepagent_runtime::workflow::code::{CodeInput, CodeOutput};

#[test]
#[ignore = "requires explicit managed SDK fixture and an unpackaged terminal; executes fixed diagnostics"]
fn h178_fixed_node_startup_diagnostic_keeps_all_production_guards() {
    for protocol in [false, true] {
        let (root, mut request) = sdk_request();
        request.code.script = "function main({ input }) { return { result: input * 7 }; }".into();
        request.code.inputs = vec![CodeInput {
            name: "input".into(),
            value: json!(6),
        }];
        request.code.outputs = vec![CodeOutput {
            name: "result".into(),
            value_type: "number".into(),
        }];
        let response = run_fixed_fixture(root.path(), &request, protocol);
        if protocol {
            assert_eq!(response.unwrap().outputs["result"], 42);
        }
    }
}

// Only fixed test scripts/inputs may call this helper: it prints native output.
pub(super) fn run_fixed_fixture(
    root: &Path,
    request: &SandboxCodeRequest,
    protocol: bool,
) -> Option<CodeResponse> {
    let label = if protocol { "adapter" } else { "startup" };
    let controls = Controls {
        request,
        abandoned: Arc::new(AtomicBool::new(false)),
        started: Instant::now(),
    };
    let stage = root.join("stage");
    std::fs::create_dir(&stage).unwrap();
    let executable = stage_runtime(request, &stage, &controls).unwrap();
    let adapter = stage.join("fixed-diagnostic.cjs");
    std::fs::write(
        &adapter,
        if protocol {
            include_str!("../../javascript.cjs")
        } else {
            "process.stdout.write('H178_TRUSTED_NODE_BOOT_OK');"
        },
    )
    .unwrap();
    let mut profile = Profile::new().unwrap();
    grant_staging_readonly(&stage, profile.sid).unwrap();
    let (input_write, stdin) = stdio_pipe(true).unwrap();
    let (output_read, stdout) = stdio_pipe(false).unwrap();
    let (error_read, stderr) = stdio_pipe(false).unwrap();
    let launch_request = Launch {
        executable: &executable,
        adapter: &adapter,
        stage: &stage,
        stdin: &stdin,
        stdout: &stdout,
        stderr: &stderr,
        profile: &profile,
        language: CodeLanguage::Javascript,
    };
    let mut capabilities = RuntimeCapabilities::for_language(request.code.language).unwrap();
    let child = launch_runtime(launch_request, capabilities.entries_mut()).unwrap();
    let identity = verify_runtime_identity(
        child.process.as_raw_handle(),
        profile.sid,
        capabilities.entries(),
    );
    println!("fixture={label} production_identity_gate={identity:?}");
    if let Err(error) = identity {
        observe_cleanup(child, &mut profile);
        panic!("must not resume a child rejected by the declared identity gate: {error}");
    }
    let observed = child.process.try_clone().unwrap();
    drop((stdin, stdout, stderr));
    let input = if protocol {
        serde_json::to_vec(&json!({
            "script":request.code.script, "inputs":request.code.input_object(),
            "logLimit":MAX_CODE_LOG_BYTES, "responseLimit":MAX_CODE_RESPONSE_BYTES,
        }))
        .unwrap()
    } else {
        Vec::new()
    };
    let outcome = communicate(
        child,
        input_write,
        output_read,
        error_read,
        input,
        &controls,
    );
    assert_eq!(
        unsafe { WaitForSingleObject(observed.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    profile.cleanup().unwrap();
    println!("fixture={label} child_terminated_and_profile_deleted=true");
    let (exit, stdout, stderr) = outcome.expect("bounded fixed diagnostic communication");
    // Safe only for these fixed fixtures. Production decode must continue
    // withholding native stderr that can contain private workflow values.
    println!("fixture={label} sdk_exit={exit} sdk_exit_hex={exit:#010x}");
    println!(
        "fixture={label} stdout_bytes={} stdout_truncated={} stdout={:?}",
        stdout.bytes.len(),
        stdout.truncated,
        String::from_utf8_lossy(&stdout.bytes)
    );
    println!(
        "fixture={label} stderr_bytes={} stderr_truncated={} stderr={:?}",
        stderr.bytes.len(),
        stderr.truncated,
        String::from_utf8_lossy(&stderr.bytes)
    );
    assert_eq!(
        exit, 0,
        "fixed {label} must start under the declared isolation"
    );
    if protocol {
        let response = response::decode(
            &request.code,
            request.runtime.version.clone(),
            CodeIsolation::WindowsLpacV1,
            exit,
            stdout,
            stderr,
        )
        .unwrap();
        Some(response)
    } else {
        assert!(!stdout.truncated);
        assert_eq!(stdout.bytes, b"H178_TRUSTED_NODE_BOOT_OK");
        None
    }
}
