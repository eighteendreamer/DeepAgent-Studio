//! Windows ownership for one local MCP service, not for one model invocation.
//!
//! Create the child suspended: assigning an already running process cannot
//! contain descendants it created before assignment. Unlike the shell tool,
//! an MCP service never preserves descendants after its last owner is dropped.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetProcessIdOfThread, OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub(super) struct ProcessJob(OwnedHandle);

impl ProcessJob {
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        Self::spawn_with_setup(command, Self::assign_and_resume)
    }

    fn spawn_with_setup(
        command: &mut Command,
        setup: impl FnOnce(&Self, &Child) -> io::Result<()>,
    ) -> io::Result<(Child, Self)> {
        // No inheritable or named job handle; a child cannot keep the owner
        // alive by inheriting it. No BREAKAWAY_OK/SILENT_BREAKAWAY_OK flags.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned a fresh, valid, owned handle.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle.cast()) });
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        command
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .kill_on_drop(true);
        let child = command.spawn()?;
        // On every error, Child's kill_on_drop and the job's last-handle close
        // terminate the still-owned child. Never resume without containment.
        setup(&job, &child)?;
        Ok((child, job))
    }

    fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("missing suspended MCP process handle"))?;
        self.assign_process(process.cast())?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("missing suspended MCP process id"))?;
        resume_child_threads(pid)
    }

    fn assign_process(&self, process: HANDLE) -> io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle().cast(), process) } == 0 {
            return Err(io::Error::other(format!(
                "assign suspended MCP process to job: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    pub(super) fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.0.as_raw_handle().cast(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) async fn wait_empty(&self) -> io::Result<()> {
        // Job signalling is not a general-purpose empty-job event. Query its
        // active count, including nested jobs, then the caller must still wait
        // for its root Child: accounting zero is not process-handle signalling.
        tokio::time::timeout(SHUTDOWN_GRACE, async {
            loop {
                let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                    unsafe { std::mem::zeroed() };
                if unsafe {
                    QueryInformationJobObject(
                        self.0.as_raw_handle().cast(),
                        JobObjectBasicAccountingInformation,
                        std::ptr::addr_of_mut!(accounting).cast(),
                        std::mem::size_of_val(&accounting) as u32,
                        std::ptr::null_mut(),
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if accounting.ActiveProcesses == 0 {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "MCP job did not become empty in 2s",
            )
        })?
    }
}

fn resume_child_threads(pid: u32) -> io::Result<()> {
    resume_child_threads_with(pid, |thread| unsafe { ResumeThread(thread) })
}

fn resume_child_threads_with(pid: u32, mut resume: impl FnMut(HANDLE) -> u32) -> io::Result<()> {
    // Rust's primary-thread handle API is nightly-only. Use documented ToolHelp
    // and ResumeThread on stable instead of an undocumented NtResumeProcess.
    // Collect handles before resuming, so threads created after startup cannot
    // enter this snapshot. Ownership was established before any thread resumes.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ToolHelp returned a valid owned snapshot handle.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.cast()) };
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of_val(&entry) as u32;
    if unsafe { Thread32First(snapshot.as_raw_handle().cast(), &mut entry) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut threads = Vec::new();
    loop {
        if entry.th32OwnerProcessID == pid {
            let thread = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    entry.th32ThreadID,
                )
            };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: OpenThread returned a valid owned handle. Verify the
            // snapshot's owner again so a recycled thread id cannot target a
            // thread outside this exact, still-held child process.
            let thread = unsafe { OwnedHandle::from_raw_handle(thread.cast()) };
            if unsafe { GetProcessIdOfThread(thread.as_raw_handle().cast()) } != pid {
                return Err(io::Error::other("MCP startup thread owner changed"));
            }
            threads.push(thread);
        }
        if unsafe { Thread32Next(snapshot.as_raw_handle().cast(), &mut entry) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                return Err(error);
            }
            break;
        }
    }
    if threads.is_empty() {
        return Err(io::Error::other(
            "suspended MCP process has no startup thread",
        ));
    }
    for thread in threads {
        let previous = resume(thread.as_raw_handle().cast());
        if previous == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        if previous > 1 {
            return Err(io::Error::other("MCP startup thread remains suspended"));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "stdio_windows_job_tests.rs"]
mod tests;
