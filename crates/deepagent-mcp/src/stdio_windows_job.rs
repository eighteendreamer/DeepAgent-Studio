//! Windows ownership for one local MCP service, not for one model invocation.
//!
//! Create the child suspended: assigning an already running process cannot
//! contain descendants it created before assignment. Unlike the shell tool,
//! an MCP service never preserves descendants after its last owner is dropped.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::{
    ERROR_MORE_DATA, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
    JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetProcessIdOfThread, OpenProcess, OpenThread, ResumeThread, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_SUSPENDED, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub(super) struct ProcessJob {
    handle: OwnedHandle,
    // Synchronous initialization cannot be cancelled between capture and the
    // termination request. Retain handles/errors across cancelled or repeated
    // close futures: an already empty job is not proof of descendant teardown.
    shutdown_members: OnceLock<io::Result<Vec<OwnedHandle>>>,
}

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
        let job = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle.cast()) },
            shutdown_members: OnceLock::new(),
        };
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
        if unsafe { AssignProcessToJobObject(self.handle.as_raw_handle().cast(), process) } == 0 {
            return Err(io::Error::other(format!(
                "assign suspended MCP process to job: {}",
                io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    pub(super) fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.handle.as_raw_handle().cast(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) async fn shutdown(&self) -> io::Result<()> {
        let members = self.begin_shutdown_with(Self::capture_shutdown_members);
        let members = members.as_ref().map_err(|error| {
            io::Error::new(error.kind(), format!("prepare MCP job shutdown: {error}"))
        })?;
        tokio::time::timeout(SHUTDOWN_GRACE, async {
            loop {
                let mut all_exited = true;
                for member in members {
                    match unsafe { WaitForSingleObject(member.as_raw_handle().cast(), 0) } {
                        WAIT_OBJECT_0 => {}
                        WAIT_TIMEOUT => all_exited = false,
                        WAIT_FAILED => return Err(io::Error::last_os_error()),
                        state => {
                            return Err(io::Error::other(format!(
                                "unexpected MCP process wait state: {state}"
                            )))
                        }
                    }
                }
                if all_exited && self.active_processes()? == 0 {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "MCP job members did not finish exiting in 2s",
            )
        })?
    }

    fn begin_shutdown_with(
        &self,
        capture: impl FnOnce(&Self) -> io::Result<Vec<OwnedHandle>>,
    ) -> &io::Result<Vec<OwnedHandle>> {
        self.shutdown_members.get_or_init(|| {
            let members = capture(self);
            // Even a capture/permission failure must request termination. Never
            // turn an unverified member into a successful close on a retry.
            let terminated = self.terminate();
            match (members, terminated) {
                (Ok(members), Ok(())) => Ok(members),
                (Err(capture), Err(termination)) => Err(io::Error::other(format!(
                    "capture MCP job members: {capture}; terminate job: {termination}"
                ))),
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        })
    }

    fn active_processes(&self) -> io::Result<u32> {
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle().cast(),
                JobObjectBasicAccountingInformation,
                std::ptr::addr_of_mut!(accounting).cast(),
                std::mem::size_of_val(&accounting) as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(accounting.ActiveProcesses)
    }

    fn capture_shutdown_members(&self) -> io::Result<Vec<OwnedHandle>> {
        // Close admission before enumerating, including nested jobs. A single
        // snapshot without this barrier races a member creating a new child.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle().cast(),
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of_mut!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 0;
        if unsafe {
            SetInformationJobObject(
                self.handle.as_raw_handle().cast(),
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        self.member_ids()?
            .into_iter()
            .map(|pid| self.open_member(pid))
            .collect()
    }

    fn open_member(&self, pid: u32) -> io::Result<OwnedHandle> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(io::Error::other(format!(
                "open MCP job member {pid}: {}",
                io::Error::last_os_error()
            )));
        }
        // SAFETY: OpenProcess returned a fresh owned handle. A PID snapshot
        // is not identity: verify this exact handle belongs to our job.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
        let mut belongs = 0;
        if unsafe {
            IsProcessInJob(
                handle.as_raw_handle().cast(),
                self.handle.as_raw_handle().cast(),
                &mut belongs,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if belongs == 0 {
            return Err(io::Error::other(format!(
                "MCP job member {pid} identity changed"
            )));
        }
        Ok(handle)
    }

    fn member_ids(&self) -> io::Result<Vec<u32>> {
        // usize storage gives the variable ULONG_PTR array its required
        // alignment on both Windows architectures; the two DWORDs precede it.
        let header_words = (2 * std::mem::size_of::<u32>()) / std::mem::size_of::<usize>();
        let mut capacity = 16usize;
        loop {
            let words = header_words
                .checked_add(capacity)
                .ok_or_else(|| io::Error::other("MCP member buffer overflow"))?;
            let bytes = words
                .checked_mul(std::mem::size_of::<usize>())
                .and_then(|bytes| u32::try_from(bytes).ok())
                .ok_or_else(|| io::Error::other("MCP member buffer exceeds Windows API size"))?;
            let mut buffer = Vec::<usize>::new();
            buffer.try_reserve_exact(words).map_err(io::Error::other)?;
            buffer.resize(words, 0);
            let list = buffer
                .as_mut_ptr()
                .cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
            let success = unsafe {
                QueryInformationJobObject(
                    self.handle.as_raw_handle().cast(),
                    JobObjectBasicProcessIdList,
                    list.cast(),
                    bytes,
                    std::ptr::null_mut(),
                )
            };
            if success == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
                    return Err(error);
                }
            }
            // SAFETY: the aligned buffer contains both fixed DWORD fields.
            let (assigned, count) = unsafe {
                (
                    (*list).NumberOfAssignedProcesses as usize,
                    (*list).NumberOfProcessIdsInList as usize,
                )
            };
            if success == 0 || count < assigned {
                capacity = capacity
                    .checked_mul(2)
                    .map(|value| value.max(assigned))
                    .ok_or_else(|| io::Error::other("MCP member capacity overflow"))?;
                continue;
            }
            if count > capacity {
                return Err(io::Error::other("invalid MCP job member count"));
            }
            return buffer[header_words..header_words + count]
                .iter()
                .map(|&pid| {
                    u32::try_from(pid).map_err(|_| io::Error::other("invalid MCP job member id"))
                })
                .collect();
        }
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
