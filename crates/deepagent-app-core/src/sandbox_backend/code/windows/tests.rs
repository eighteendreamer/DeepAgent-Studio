use super::process::verify_package_opt_out;
use super::profile::sid_string;
use super::*;
use crate::runtime_service::{RuntimeBroker, RuntimeService, UnavailableDownloader};
use deepagent_runtime::workflow::code::{CodeRequest, CODE_VERSION};
use deepagent_tools::ToolExecutionContext;
use std::mem::size_of;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenAccessInformation, TokenCapabilities, TokenIsAppContainer,
    TokenIsLessPrivilegedAppContainer, TOKEN_ACCESS_INFORMATION, TOKEN_DUPLICATE, TOKEN_GROUPS,
    TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::OpenProcessToken;

#[path = "tests/acceptance.rs"]
mod acceptance;
#[path = "tests/capability.rs"]
mod capability;
#[path = "tests/startup.rs"]
mod startup;

fn sdk_request() -> (tempfile::TempDir, SandboxCodeRequest) {
    let lookup = PathBuf::from(
        std::env::var_os("H178_MANAGED_SDK_FIXTURE_ROOT").expect("explicit SDK fixture required"),
    );
    let root = tempfile::tempdir().unwrap();
    let service = Arc::new(RuntimeService::with_lookup_roots(
        &[root.path().to_path_buf()],
        &[lookup],
        Arc::new(UnavailableDownloader),
    ));
    let runtime = RuntimeBroker::new(service.clone())
        .resolve(
            &service.managed_requirement(RuntimeKind::Node).unwrap(),
            None,
            None,
        )
        .unwrap();
    (
        root,
        SandboxCodeRequest {
            code: CodeRequest {
                version: CODE_VERSION,
                language: CodeLanguage::Javascript,
                script: "function main() { return {}; }".into(),
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            runtime,
            context: ToolExecutionContext::default(),
        },
    )
}

#[test]
#[ignore = "requires explicit SDK fixture; only inspects a suspended child, never executes source"]
fn h178_lpac_suspended_identity_probe() {
    let (_root, mut profile, child) = suspended_sdk(true);
    let mut parent_raw = null_mut();
    checked(
        unsafe {
            OpenProcessToken(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                TOKEN_QUERY,
                &mut parent_raw,
            )
        },
        "inspect parent token",
    )
    .unwrap();
    let parent = handle(parent_raw, "parent token").unwrap();
    probe_capabilities("parent", &parent);
    let token = child_token(&child);
    let child_capabilities = probe_capabilities("child", &token);
    println!("requested_profile_sid={}", sid_string(profile.sid).unwrap());
    for class in [
        TokenIsAppContainer,
        TokenIsLessPrivilegedAppContainer,
        TokenCapabilities,
        TokenAccessInformation,
    ] {
        let mut length = 0;
        let first = unsafe {
            GetTokenInformation(token.as_raw_handle(), class, null_mut(), 0, &mut length)
        };
        let first_error = if first == 0 {
            Some(unsafe { GetLastError() })
        } else {
            None
        };
        let mut storage = vec![0usize; (length.max(4096) as usize).div_ceil(size_of::<usize>())];
        let second = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                class,
                storage.as_mut_ptr().cast(),
                (storage.len() * size_of::<usize>()) as u32,
                &mut length,
            )
        };
        let second_error = if second == 0 {
            Some(unsafe { GetLastError() })
        } else {
            None
        };
        println!("class={class} sizing_ok={first} sizing_error={first_error:?} query_ok={second} query_error={second_error:?} returned_bytes={length}");
        if second != 0 && class == TokenCapabilities && length as usize >= size_of::<TOKEN_GROUPS>()
        {
            let groups = unsafe { &*storage.as_ptr().cast::<TOKEN_GROUPS>() };
            println!("capability_count={}", groups.GroupCount);
            for n in 0..groups.GroupCount as usize {
                let sid = unsafe { *groups.Groups.as_ptr().add(n) };
                println!(
                    "capability_sid={} attributes={:#x}",
                    sid_string(sid.Sid).unwrap(),
                    sid.Attributes
                );
            }
        }
        if second != 0
            && class == TokenAccessInformation
            && length as usize >= size_of::<TOKEN_ACCESS_INFORMATION>()
        {
            let info = unsafe { &*storage.as_ptr().cast::<TOKEN_ACCESS_INFORMATION>() };
            let count = if info.CapabilitiesHash.is_null() {
                None
            } else {
                Some(unsafe { (*info.CapabilitiesHash).SidCount })
            };
            println!("access_information_capability_count={count:?}");
        }
    }
    println!(
        "effective_aap_access_check={:?}",
        verify_package_opt_out(&token, profile.sid)
    );
    let identity = verify_identity(child.process.as_raw_handle(), profile.sid);
    println!("zero_capability_control_identity_gate={identity:?}");
    if child_capabilities != 0 {
        assert!(identity
            .unwrap_err()
            .to_string()
            .contains("unexpected SDK capability"));
    }
    drop(token);
    observe_cleanup(child, &mut profile);
}

#[test]
#[ignore = "requires explicit SDK fixture; both control children stay suspended"]
fn h178_aap_access_check_distinguishes_opt_out_from_ordinary_appcontainer() {
    for opt_out in [false, true] {
        let (_root, mut profile, child) = suspended_sdk(opt_out);
        let token = child_token(&child);
        let result = verify_package_opt_out(&token, profile.sid);
        println!("requested_opt_out={opt_out} effective_opt_out_check={result:?}");
        drop(token);
        observe_cleanup(child, &mut profile);
        if opt_out {
            result.unwrap();
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("retained ambient application package access"));
        }
    }
}

fn suspended_sdk(opt_out: bool) -> (tempfile::TempDir, Profile, Child) {
    let (root, request) = sdk_request();
    let controls = Controls {
        request: &request,
        abandoned: Arc::new(AtomicBool::new(false)),
        started: Instant::now(),
    };
    let stage = root.path().join("stage");
    std::fs::create_dir(&stage).unwrap();
    let executable = stage_runtime(&request, &stage, &controls).unwrap();
    let adapter = stage.join("never-executed.cjs");
    std::fs::write(
        &adapter,
        "throw new Error('this process must remain suspended');",
    )
    .unwrap();
    let profile = Profile::new().unwrap();
    grant_staging_readonly(&stage, profile.sid).unwrap();
    let (_input, stdin) = stdio_pipe(true).unwrap();
    let (_out, stdout) = stdio_pipe(false).unwrap();
    let (_err, stderr) = stdio_pipe(false).unwrap();
    let request = Launch {
        executable: &executable,
        adapter: &adapter,
        stage: &stage,
        stdin: &stdin,
        stdout: &stdout,
        stderr: &stderr,
        profile: &profile,
        language: CodeLanguage::Javascript,
    };
    let child = if opt_out {
        launch(request)
    } else {
        process::launch_aap_control(request)
    }
    .unwrap();
    (root, profile, child)
}

fn child_token(child: &Child) -> OwnedHandle {
    let mut raw = null_mut();
    checked(
        unsafe {
            OpenProcessToken(
                child.process.as_raw_handle(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut raw,
            )
        },
        "probe token",
    )
    .unwrap();
    handle(raw, "probe token").unwrap()
}

fn observe_cleanup(child: Child, profile: &mut Profile) {
    assert_eq!(
        unsafe { WaitForSingleObject(child.process.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
    let observed_process = child.process.try_clone().unwrap();
    drop(child);
    assert_eq!(
        unsafe { WaitForSingleObject(observed_process.as_raw_handle(), 5000) },
        WAIT_OBJECT_0
    );
    profile.cleanup().unwrap();
    println!("suspended_child_terminated_and_profile_deleted=true; no script executed");
}

fn probe_capabilities(label: &str, token: &OwnedHandle) -> u32 {
    let mut length = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenCapabilities,
            null_mut(),
            0,
            &mut length,
        )
    };
    let mut storage = vec![
        0usize;
        (length.max(size_of::<TOKEN_GROUPS>() as u32) as usize)
            .div_ceil(size_of::<usize>())
    ];
    checked(
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenCapabilities,
                storage.as_mut_ptr().cast(),
                (storage.len() * size_of::<usize>()) as u32,
                &mut length,
            )
        },
        "probe capability list",
    )
    .unwrap();
    let count = unsafe { *storage.as_ptr().cast::<u32>() };
    println!("{label}_capability_count={count} returned_bytes={length}");
    if count > 0 {
        let groups = unsafe { &*storage.as_ptr().cast::<TOKEN_GROUPS>() };
        for n in 0..count as usize {
            let entry = unsafe { *groups.Groups.as_ptr().add(n) };
            println!(
                "{label}_capability={} attributes={:#x}",
                sid_string(entry.Sid).unwrap(),
                entry.Attributes
            );
        }
    }
    let mut length = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenAccessInformation,
            null_mut(),
            0,
            &mut length,
        )
    };
    let mut storage = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    checked(
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenAccessInformation,
                storage.as_mut_ptr().cast(),
                (storage.len() * size_of::<usize>()) as u32,
                &mut length,
            )
        },
        "probe package identity",
    )
    .unwrap();
    let info = unsafe { &*storage.as_ptr().cast::<TOKEN_ACCESS_INFORMATION>() };
    let package = if info.PackageSid.is_null() {
        None
    } else {
        Some(sid_string(info.PackageSid).unwrap())
    };
    println!("{label}_package_sid={package:?}");
    count
}
