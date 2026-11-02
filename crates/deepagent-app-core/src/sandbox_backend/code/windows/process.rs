//! Windows LPAC launch and verification. Successful attribute updates are
//! mandatory, but never substitute for checking the actual suspended child.
//! https://learn.microsoft.com/windows/win32/secauthz/implementing-an-appcontainer
use super::profile::{sid_string, LocalAllocation, Profile};
use super::{checked, handle, os_error, wide};
use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::workflow::code::CodeLanguage;
use std::ffi::OsStr;
use std::fs::File;
use std::mem::{size_of, size_of_val, zeroed};
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::{
    AccessCheck, DuplicateToken, EqualSid, GetTokenInformation, SecurityImpersonation,
    TokenAppContainerSid, TokenIsAppContainer, GENERIC_MAPPING, PRIVILEGE_SET, PSID,
    SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, TOKEN_APPCONTAINER_INFORMATION,
    TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    OpenProcessToken, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use windows_sys::Win32::System::WindowsProgramming::PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
const PROCESS_MEMORY_BYTES: usize = 256 * 1024 * 1024;
// Windows command line quoting for exact, trusted adapter argv. Source/inputs
// are stdin JSON, never part of this string. No shell interprets this argv.
fn quote(value: &OsStr) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for c in value.to_string_lossy().chars() {
        if c == '\\' {
            slashes += 1;
        } else {
            result.extend(std::iter::repeat('\\').take(slashes * if c == '"' { 2 } else { 1 }));
            slashes = 0;
            if c == '"' {
                result.push('\\');
            }
            result.push(c);
        }
    }
    result.extend(std::iter::repeat('\\').take(slashes * 2));
    result.push('"');
    result
}

#[cfg(test)]
#[test]
fn exact_argv_quoting_keeps_spaces_unicode_and_trailing_backslashes() {
    // Microsoft CRT argument parsing, not shell quoting:
    // https://learn.microsoft.com/cpp/c-language/parsing-c-command-line-arguments
    assert_eq!(quote(OsStr::new("")), "\"\"");
    assert_eq!(
        quote(OsStr::new("C:\\SDK 空格\\entry.cjs")),
        "\"C:\\SDK 空格\\entry.cjs\""
    );
    assert_eq!(quote(OsStr::new("C:\\SDK path\\")), "\"C:\\SDK path\\\\\"");
    assert_eq!(quote(OsStr::new("a\"b")), "\"a\\\"b\"");
}

struct Attributes(Vec<usize>);
impl Attributes {
    fn new(count: u32) -> Result<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut bytes) };
        if bytes == 0 {
            return Err(os_error("size attributes"));
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // Do not construct the Drop owner before initialization succeeds.
        checked(
            unsafe {
                InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), count, 0, &mut bytes)
            },
            "initialize attributes",
        )?;
        Ok(Self(storage))
    }

    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }

    // SAFETY at each call: pointed-to storage outlives the list and CreateProcess.
    unsafe fn set<T>(&mut self, attribute: u32, value: &T) -> Result<()> {
        checked(
            unsafe {
                UpdateProcThreadAttribute(
                    self.ptr(),
                    0,
                    attribute as usize,
                    (value as *const T).cast_mut().cast(),
                    size_of_val(value),
                    null_mut(),
                    null_mut(),
                )
            },
            "set process attribute",
        )
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

pub(super) fn stdio_pipe(child_reads: bool) -> Result<(File, OwnedHandle)> {
    let (mut read, mut write) = (null_mut(), null_mut());
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    checked(
        unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) },
        "create output pipe",
    )?;
    let reader = handle(read, "pipe reader")?;
    let writer = handle(write, "pipe writer")?;
    let (parent, child) = if child_reads {
        (writer, reader)
    } else {
        (reader, writer)
    };
    checked(
        unsafe { SetHandleInformation(parent.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) },
        "protect parent pipe",
    )?;
    Ok((File::from(parent), child))
}

pub(super) struct Child {
    pub(super) process: OwnedHandle,
    pub(super) thread: OwnedHandle,
    job: OwnedHandle,
}
impl Drop for Child {
    fn drop(&mut self) {
        // Also used on cancellation, dropped futures, invalid identity and
        // failed resume. A job is never shared with any other run/backend.
        unsafe {
            TerminateJobObject(self.job.as_raw_handle(), 1);
            WaitForSingleObject(self.process.as_raw_handle(), 5000);
        }
    }
}

fn job() -> Result<OwnedHandle> {
    let job = handle(unsafe { CreateJobObjectW(null(), null()) }, "create job")?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = PROCESS_MEMORY_BYTES;
    checked(
        unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            )
        },
        "set job limits",
    )?;
    Ok(job)
}

#[cfg(test)]
pub(super) fn assert_child_job_limits(child: &Child) {
    use windows_sys::Win32::System::JobObjects::{IsProcessInJob, QueryInformationJobObject};
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    let mut length = 0;
    checked(
        unsafe {
            QueryInformationJobObject(
                child.job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&mut limits as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
                &mut length,
            )
        },
        "query actual SDK job limits",
    )
    .unwrap();
    assert_eq!(length as usize, size_of_val(&limits));
    assert_eq!(limits.ProcessMemoryLimit, PROCESS_MEMORY_BYTES);
    assert_eq!(limits.BasicLimitInformation.ActiveProcessLimit, 1);
    assert_eq!(
        limits.BasicLimitInformation.LimitFlags,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
    );
    let mut contained = 0;
    checked(
        unsafe {
            IsProcessInJob(
                child.process.as_raw_handle(),
                child.job.as_raw_handle(),
                &mut contained,
            )
        },
        "query actual SDK job membership",
    )
    .unwrap();
    assert_ne!(contained, 0);
}

#[cfg(test)]
pub(super) fn verify_identity(process: HANDLE, expected_sid: PSID) -> Result<()> {
    verify_declared_identity(process, expected_sid, &[])
}

// All paths use the same package/AAP gates and exact capability comparison.
fn verify_declared_identity(
    process: HANDLE,
    expected_sid: PSID,
    capabilities: &[SID_AND_ATTRIBUTES],
) -> Result<()> {
    let mut raw = null_mut();
    checked(
        unsafe { OpenProcessToken(process, TOKEN_QUERY | TOKEN_DUPLICATE, &mut raw) },
        "inspect SDK token",
    )?;
    let token = handle(raw, "SDK token")?;
    {
        let (mut flag, mut length) = (0u32, 0u32);
        checked(
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenIsAppContainer,
                    (&mut flag as *mut u32).cast(),
                    size_of::<u32>() as u32,
                    &mut length,
                )
            },
            "verify LPAC token",
        )?;
        if flag == 0 {
            return Err(CoreError::other(
                "CodeIsolationFailed: SDK token is not an AppContainer",
            ));
        }
    }
    let mut info: TOKEN_APPCONTAINER_INFORMATION = unsafe { zeroed() };
    let mut length = 0;
    // The SID is variable-sized; query into aligned storage owned until comparison.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenAppContainerSid,
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
                TokenAppContainerSid,
                storage.as_mut_ptr().cast(),
                (storage.len() * size_of::<usize>()) as u32,
                &mut length,
            )
        },
        "verify invocation SID",
    )?;
    if storage.len() * size_of::<usize>() < size_of_val(&info) {
        return Err(CoreError::other("CodeIsolationFailed: truncated SDK token"));
    }
    info = unsafe { *(storage.as_ptr().cast::<TOKEN_APPCONTAINER_INFORMATION>()) };
    if unsafe { EqualSid(info.TokenAppContainer, expected_sid) } == 0 {
        return Err(CoreError::other(
            "CodeIsolationFailed: SDK invocation SID mismatch",
        ));
    }
    super::capabilities::verify(token.as_raw_handle(), capabilities)?;
    verify_package_opt_out(&token, expected_sid)
}

pub(super) fn verify_package_opt_out(token: &OwnedHandle, expected_sid: PSID) -> Result<()> {
    // Class 46 has no complete Win32 output contract and actually returns
    // ERROR_INVALID_PARAMETER on the tested OS. Never interpret a failed query
    // as LPAC. Check both sides of the documented dual-principal access model:
    // Everyone satisfies the ordinary user/group check; a second ACE must
    // satisfy the package check. AAP alone also denies ordinary AppContainers
    // on the tested OS and therefore cannot demonstrate opt-out.
    let mut raw = null_mut();
    checked(
        unsafe { DuplicateToken(token.as_raw_handle(), SecurityImpersonation, &mut raw) },
        "duplicate SDK token for access check",
    )?;
    let impersonation = handle(raw, "SDK impersonation token")?;
    let positive = format!(
        "O:SYG:SYD:(A;;0x1;;;WD)(A;;0x1;;;{})",
        sid_string(expected_sid)?
    );
    if !descriptor_grants(impersonation.as_raw_handle(), &positive)? {
        return Err(CoreError::other(
            "CodeIsolationFailed: invocation access positive control was denied",
        ));
    }
    if descriptor_grants(
        impersonation.as_raw_handle(),
        "O:SYG:SYD:(A;;0x1;;;WD)(A;;0x1;;;S-1-15-2-1)",
    )? {
        return Err(CoreError::other(
            "CodeIsolationFailed: SDK retained ambient application package access",
        ));
    }
    Ok(())
}

fn descriptor_grants(impersonation: HANDLE, sddl: &str) -> Result<bool> {
    let sddl = wide(sddl)?;
    let mut descriptor = null_mut();
    checked(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        },
        "build AAP access probe",
    )?;
    let _allocation = LocalAllocation(descriptor);
    let mapping = GENERIC_MAPPING {
        GenericRead: 1,
        GenericWrite: 2,
        GenericExecute: 4,
        GenericAll: 7,
    };
    let mut privileges: PRIVILEGE_SET = unsafe { zeroed() };
    let mut bytes = size_of_val(&privileges) as u32;
    let (mut granted, mut allowed) = (0, 0);
    checked(
        unsafe {
            AccessCheck(
                descriptor,
                impersonation,
                1,
                &mapping,
                &mut privileges,
                &mut bytes,
                &mut granted,
                &mut allowed,
            )
        },
        "check SDK descriptor access",
    )?;
    Ok(allowed != 0 && granted & 1 != 0)
}

pub(super) struct Launch<'a> {
    pub executable: &'a Path,
    pub adapter: &'a Path,
    pub stage: &'a Path,
    pub stdin: &'a OwnedHandle,
    pub stdout: &'a OwnedHandle,
    pub stderr: &'a OwnedHandle,
    pub profile: &'a Profile,
    pub language: CodeLanguage,
}

enum EntryPath {
    #[cfg(test)]
    Canonical,
    PreserveStagedMain,
}

fn runtime_flags(language: CodeLanguage, entry_path: EntryPath) -> &'static str {
    match (language, entry_path) {
        #[cfg(test)]
        (CodeLanguage::Javascript, EntryPath::Canonical) => "--no-addons --max-old-space-size=128",
        (CodeLanguage::Javascript, EntryPath::PreserveStagedMain) => {
            // Node otherwise realpath/lstats every ancestor, including C:\,
            // before reading our fixed .cjs entrypoint. Do not grant those
            // ancestors: use its documented main-only loader option.
            // The invocation's adapter is freshly created, not a user path;
            // OS isolation and staging DACLs remain the security boundary.
            // https://nodejs.org/download/release/v24.21.0/docs/api/cli.html#--preserve-symlinks-main
            "--no-addons --max-old-space-size=128 --preserve-symlinks-main"
        }
        (CodeLanguage::Python, _) => "-I -B -X utf8",
    }
}

#[cfg(test)]
#[test]
fn staged_main_option_keeps_no_addons_and_memory_flags_without_broad_symlink_option() {
    assert_eq!(
        runtime_flags(CodeLanguage::Javascript, EntryPath::Canonical),
        "--no-addons --max-old-space-size=128"
    );
    assert_eq!(
        runtime_flags(CodeLanguage::Javascript, EntryPath::PreserveStagedMain),
        "--no-addons --max-old-space-size=128 --preserve-symlinks-main"
    );
    for entry in [EntryPath::Canonical, EntryPath::PreserveStagedMain] {
        assert_eq!(runtime_flags(CodeLanguage::Python, entry), "-I -B -X utf8");
    }
}

#[cfg(test)]
pub(super) fn launch(request: Launch<'_>) -> Result<Child> {
    launch_with_package_opt_out(request, true, &mut [], EntryPath::Canonical)
}

#[cfg(test)]
pub(super) fn launch_aap_control(request: Launch<'_>) -> Result<Child> {
    // Suspended identity-control tests only; never a production SDK fallback.
    launch_with_package_opt_out(request, false, &mut [], EntryPath::Canonical)
}

pub(super) fn launch_runtime(
    request: Launch<'_>,
    capabilities: &mut [SID_AND_ATTRIBUTES],
) -> Result<Child> {
    launch_with_package_opt_out(request, true, capabilities, EntryPath::PreserveStagedMain)
}

pub(super) fn verify_runtime_identity(
    process: HANDLE,
    expected_sid: PSID,
    capabilities: &[SID_AND_ATTRIBUTES],
) -> Result<()> {
    verify_declared_identity(process, expected_sid, capabilities)
}

fn launch_with_package_opt_out(
    request: Launch<'_>,
    opt_out: bool,
    declared: &mut [SID_AND_ATTRIBUTES],
    entry_path: EntryPath,
) -> Result<Child> {
    let Launch {
        executable,
        adapter,
        stage,
        stdin,
        stdout,
        stderr,
        profile,
        language,
    } = request;
    let job = job()?;
    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: profile.sid,
        Capabilities: if declared.is_empty() {
            null_mut()
        } else {
            declared.as_mut_ptr()
        },
        CapabilityCount: declared.len() as u32,
        Reserved: 0,
    };
    let policy = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
    let handles = [
        stdin.as_raw_handle(),
        stdout.as_raw_handle(),
        stderr.as_raw_handle(),
    ];
    let jobs = [job.as_raw_handle()];
    let mut attributes = Attributes::new(if opt_out { 4 } else { 3 })?;
    unsafe {
        attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, &capabilities)?;
        if opt_out {
            attributes.set(
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
                &policy,
            )?;
        }
        attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
        attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    }
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of_val(&startup) as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.ptr();
    let flags = runtime_flags(language, entry_path);
    let mut command = wide(format!(
        "{} {flags} {}",
        quote(executable.as_os_str()),
        quote(adapter.as_os_str())
    ))?;
    let executable = wide(executable)?;
    let cwd = wide(stage)?;
    // Trusted SystemRoot is queried from the OS, not inherited env/PATH. No
    // NODE_OPTIONS, PYTHONPATH, proxy, credential or user package environment.
    let mut system_root = vec![0u16; 32768];
    let length =
        unsafe { GetWindowsDirectoryW(system_root.as_mut_ptr(), system_root.len() as u32) };
    if length == 0 || length as usize >= system_root.len() {
        return Err(os_error("query system root"));
    }
    system_root.truncate(length as usize);
    // AppContainer process creation requires LOCALAPPDATA (also retained by
    // Chromium's filtered launch environment). Supply only this invocation's
    // new profile location, not the caller's user environment.
    let local = profile.folder()?;
    let mut environment = Vec::new();
    for entry in [
        format!("LOCALAPPDATA={local}"),
        format!("SystemRoot={}", String::from_utf16_lossy(&system_root)),
        format!("TEMP={local}\\Temp"),
        format!("TMP={local}\\Temp"),
    ] {
        environment.extend(wide(entry)?);
    }
    environment.push(0);
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    checked(
        unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED
                    | CREATE_NO_WINDOW
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut process,
            )
        },
        "start LPAC SDK",
    )?;
    let child = Child {
        process: handle(process.hProcess, "SDK process")?,
        thread: handle(process.hThread, "SDK thread")?,
        job,
    };
    Ok(child)
}
