//! Strict SDK orchestration: controls, bounded staging and stream capture.
//! Win32 ownership/ACL and process setup live in private profile/process modules.
use super::super::SandboxCodeRequest;
use super::response::{self, Capture};
use crate::runtime_service::{RuntimeKind, RuntimeSource};
use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::workflow::code::{
    CodeIsolation, CodeLanguage, CodeResponse, MAX_CODE_LOG_BYTES, MAX_CODE_RESPONSE_BYTES,
};
use serde_json::json;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, ResumeThread, WaitForSingleObject,
};
mod capabilities;
mod process;
mod profile;
use capabilities::RuntimeCapabilities;
#[cfg(test)]
use process::{launch, verify_identity};
use process::{launch_runtime, stdio_pipe, verify_runtime_identity, Child, Launch};
use profile::{grant_staging_readonly, Profile};
const SDK_STAGING_BYTES: u64 = 512 * 1024 * 1024;
fn os_error(operation: &str) -> CoreError {
    CoreError::other(format!(
        "CodeIsolationFailed: {operation}: {}",
        std::io::Error::last_os_error()
    ))
}

fn checked(ok: i32, operation: &str) -> Result<()> {
    if ok == 0 {
        Err(os_error(operation))
    } else {
        Ok(())
    }
}

fn wide(value: impl AsRef<OsStr>) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = value.as_ref().encode_wide().collect();
    if value.contains(&0) {
        return Err(CoreError::invalid("CodeIsolationFailed: embedded NUL"));
    }
    value.push(0);
    Ok(value)
}

fn handle(raw: HANDLE, operation: &str) -> Result<OwnedHandle> {
    if raw.is_null() {
        return Err(os_error(operation));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

struct Controls<'a> {
    request: &'a SandboxCodeRequest,
    abandoned: Arc<AtomicBool>,
    started: Instant,
}
impl Controls<'_> {
    fn check(&self) -> Result<()> {
        if self.request.context.is_cancelled() || self.abandoned.load(Ordering::Acquire) {
            return Err(CoreError::other(
                "CodeExecutionCancelled: SDK invocation cancelled",
            ));
        }
        if self.started.elapsed() >= self.request.context.timeout() {
            return Err(CoreError::other(
                "CodeExecutionTimeout: SDK invocation timed out",
            ));
        }
        Ok(())
    }
}

fn stage_runtime(
    request: &SandboxCodeRequest,
    stage: &Path,
    controls: &Controls<'_>,
) -> Result<PathBuf> {
    let target = stage.join(match request.code.language {
        CodeLanguage::Javascript => "node.exe",
        CodeLanguage::Python => "python.exe",
    });
    let mut remaining = SDK_STAGING_BYTES;
    if request.code.language == CodeLanguage::Javascript {
        copy_file(&request.runtime.executable, &target, &mut remaining)?;
    } else {
        copy_python(
            &request.runtime.root,
            stage,
            &mut remaining,
            &mut 4096,
            0,
            controls,
        )?;
    }
    controls.check()?;
    Ok(target)
}

fn copy_file(source: &Path, target: &Path, remaining: &mut u64) -> Result<()> {
    let meta = std::fs::symlink_metadata(source)
        .map_err(|_| CoreError::other("CodeRuntimeUnavailable: SDK file metadata"))?;
    if !meta.is_file()
        || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || meta.len() > *remaining
    {
        return Err(CoreError::invalid(
            "CodeRuntimeInvalid: reparse/non-file/oversized SDK",
        ));
    }
    *remaining -= meta.len();
    std::fs::copy(source, target)
        .map_err(|_| CoreError::other("CodeRuntimeUnavailable: stage SDK file"))?;
    Ok(())
}

fn copy_python(
    source: &Path,
    target: &Path,
    remaining: &mut u64,
    entries: &mut usize,
    depth: usize,
    controls: &Controls<'_>,
) -> Result<()> {
    if depth > 16 {
        return Err(CoreError::invalid("CodeRuntimeInvalid: SDK tree too deep"));
    }
    let items = std::fs::read_dir(source)
        .map_err(|_| CoreError::other("CodeRuntimeUnavailable: read SDK directory"))?;
    for item in items {
        controls.check()?;
        if *entries == 0 {
            return Err(CoreError::invalid(
                "CodeRuntimeInvalid: too many SDK entries",
            ));
        }
        *entries -= 1;
        let item = item.map_err(|_| CoreError::other("CodeRuntimeUnavailable: read SDK entry"))?;
        let meta = std::fs::symlink_metadata(item.path())
            .map_err(|_| CoreError::other("CodeRuntimeUnavailable: SDK entry metadata"))?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(CoreError::invalid("CodeRuntimeInvalid: SDK reparse point"));
        }
        let destination = target.join(item.file_name());
        if meta.is_dir() {
            std::fs::create_dir(&destination)
                .map_err(|_| CoreError::other("CodeRuntimeUnavailable: stage SDK directory"))?;
            copy_python(
                &item.path(),
                &destination,
                remaining,
                entries,
                depth + 1,
                controls,
            )?;
        } else {
            copy_file(&item.path(), &destination, remaining)?;
        }
    }
    Ok(())
}

fn drain(mut file: File, limit: usize, overflow: Option<Arc<AtomicBool>>) -> Result<Capture> {
    let mut result = Capture {
        bytes: Vec::new(),
        truncated: false,
    };
    let mut chunk = [0u8; 8192];
    loop {
        let size = file
            .read(&mut chunk)
            .map_err(|_| CoreError::other("CodeExecutionFailed: read SDK pipe"))?;
        if size == 0 {
            return Ok(result);
        }
        let accepted = size.min(limit - result.bytes.len());
        result.bytes.extend_from_slice(&chunk[..accepted]);
        if accepted < size {
            result.truncated = true;
            if let Some(flag) = &overflow {
                flag.store(true, Ordering::Release);
            }
        }
    }
}

pub(super) fn execute(
    request: SandboxCodeRequest,
    abandoned: Arc<AtomicBool>,
) -> Result<CodeResponse> {
    execute_observed(request, abandoned, |_, _, _| {})
}

// Private observation seam, not an alternate executor: acceptance can retain a
// handle to this exact verified/resumed child and check cleanup after errors.
fn execute_observed(
    request: SandboxCodeRequest,
    abandoned: Arc<AtomicBool>,
    on_resumed: impl FnOnce(&Child, &Path, &Profile),
) -> Result<CodeResponse> {
    let controls = Controls {
        request: &request,
        abandoned,
        started: Instant::now(),
    };
    controls.check()?;
    let expected_kind = match request.code.language {
        CodeLanguage::Javascript => RuntimeKind::Node,
        CodeLanguage::Python => RuntimeKind::Python,
    };
    if request.runtime.source != RuntimeSource::Managed || request.runtime.kind != expected_kind {
        return Err(CoreError::invalid(
            "CodeRuntimeInvalid: strict code requires the resolved managed SDK",
        ));
    }
    let temp = tempfile::Builder::new()
        .prefix("deepagent-code-")
        .tempdir()
        .map_err(|_| CoreError::other("CodeIsolationFailed: create invocation staging"))?;
    let stage = temp.path().join("stage");
    std::fs::create_dir(&stage)
        .map_err(|_| CoreError::other("CodeIsolationFailed: create SDK staging"))?;
    let executable = stage_runtime(&request, &stage, &controls)?;
    let adapter = stage.join(match request.code.language {
        CodeLanguage::Javascript => "entry.cjs",
        CodeLanguage::Python => "entry.py",
    });
    std::fs::write(
        &adapter,
        match request.code.language {
            CodeLanguage::Javascript => include_str!("javascript.cjs"),
            CodeLanguage::Python => include_str!("python.py"),
        },
    )
    .map_err(|_| CoreError::other("CodeIsolationFailed: stage entrypoint"))?;
    let input = serde_json::to_vec(&json!({
        "script":request.code.script, "inputs":request.code.input_object(),
        "logLimit":MAX_CODE_LOG_BYTES, "responseLimit":MAX_CODE_RESPONSE_BYTES,
    }))?;
    // Source and inputs remain in memory and this inherited pipe; never put
    // private workflow data in a staging file, argv or process environment.
    let (stdin_write, stdin) = stdio_pipe(true)?;
    let mut profile = Profile::new()?;
    grant_staging_readonly(&stage, profile.sid)?;
    let (stdout_read, stdout_write) = stdio_pipe(false)?;
    let (stderr_read, stderr_write) = stdio_pipe(false)?;
    controls.check()?;
    let mut capabilities = RuntimeCapabilities::for_language(request.code.language)?;
    let child = launch_runtime(
        Launch {
            executable: &executable,
            adapter: &adapter,
            stage: &stage,
            stdin: &stdin,
            stdout: &stdout_write,
            stderr: &stderr_write,
            profile: &profile,
            language: request.code.language,
        },
        capabilities.entries_mut(),
    )?;
    verify_runtime_identity(
        child.process.as_raw_handle(),
        profile.sid,
        capabilities.entries(),
    )?;
    drop((stdin, stdout_write, stderr_write));
    let (exit, stdout, stderr) = communicate_observed(
        child,
        stdin_write,
        stdout_read,
        stderr_read,
        input,
        &controls,
        |child| on_resumed(child, &stage, &profile),
    )?;
    profile.cleanup()?;
    controls.check()?;
    let response = response::decode(
        &request.code,
        request.runtime.version.clone(),
        CodeIsolation::WindowsLpacV1,
        exit,
        stdout,
        stderr,
    )?;
    temp.close()
        .map_err(|_| CoreError::other("CodeIsolationFailed: delete invocation staging"))?;
    Ok(response)
}

#[cfg(test)]
fn communicate(
    child: Child,
    stdin_write: File,
    stdout_read: File,
    stderr_read: File,
    input: Vec<u8>,
    controls: &Controls<'_>,
) -> Result<(u32, Capture, Capture)> {
    communicate_observed(
        child,
        stdin_write,
        stdout_read,
        stderr_read,
        input,
        controls,
        |_| {},
    )
}

fn communicate_observed(
    child: Child,
    mut stdin_write: File,
    stdout_read: File,
    stderr_read: File,
    input: Vec<u8>,
    controls: &Controls<'_>,
    on_resumed: impl FnOnce(&Child),
) -> Result<(u32, Capture, Capture)> {
    let overflow = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| -> Result<_> {
        let output_overflow = overflow.clone();
        let stdin = scope.spawn(move || stdin_write.write_all(&input));
        let stdout =
            scope.spawn(move || drain(stdout_read, MAX_CODE_RESPONSE_BYTES, Some(output_overflow)));
        let stderr = scope.spawn(move || drain(stderr_read, MAX_CODE_LOG_BYTES, None));
        let outcome = (|| -> Result<u32> {
            if unsafe { ResumeThread(child.thread.as_raw_handle()) } == u32::MAX {
                return Err(os_error("resume SDK"));
            }
            on_resumed(&child);
            loop {
                controls.check()?;
                if overflow.load(Ordering::Acquire) {
                    return Err(CoreError::other(
                        "CodeResultInvalid: SDK response exceeds limit",
                    ));
                }
                match unsafe { WaitForSingleObject(child.process.as_raw_handle(), 10) } {
                    WAIT_OBJECT_0 => break,
                    WAIT_TIMEOUT => {}
                    _ => return Err(os_error("wait for SDK")),
                }
            }
            let mut exit = 0;
            checked(
                unsafe { GetExitCodeProcess(child.process.as_raw_handle(), &mut exit) },
                "read SDK exit",
            )?;
            Ok(exit)
        })();
        drop(child); // closes all child pipe writers even on error, before join
        let input_result = stdin
            .join()
            .map_err(|_| CoreError::other("CodeExecutionFailed: stdin worker failed"))?;
        let stdout = stdout
            .join()
            .map_err(|_| CoreError::other("CodeExecutionFailed: stdout worker failed"))??;
        let stderr = stderr
            .join()
            .map_err(|_| CoreError::other("CodeExecutionFailed: stderr worker failed"))??;
        let exit = outcome?;
        input_result.map_err(|_| {
            CoreError::other(format!(
                "CodeExecutionFailed: write SDK input (exit={exit})"
            ))
        })?;
        Ok((exit, stdout, stderr))
    })
}

#[cfg(test)]
mod tests;
