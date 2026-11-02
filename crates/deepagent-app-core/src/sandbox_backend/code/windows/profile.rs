//! Invocation-local profile and read-only staging DACL ownership.
use super::{checked, wide};
use deepagent_core::error::{CoreError, Result};
use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, GetAppContainerFolderPath,
};
use windows_sys::Win32::Security::{
    FreeSid, GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSID,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
pub(super) struct Profile {
    name: Vec<u16>,
    pub(super) sid: PSID,
    deleted: bool,
}

impl Profile {
    pub(super) fn new() -> Result<Self> {
        let name = wide(format!("DeepAgent.Code.{}", uuid::Uuid::new_v4().simple()))?;
        let mut sid = null_mut();
        // SAFETY: valid terminated names/out pointer; zero capabilities. A
        // collision is an error: we never reuse/delete an existing profile.
        let status = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                name.as_ptr(),
                name.as_ptr(),
                null(),
                0,
                &mut sid,
            )
        };
        if status < 0 {
            return Err(CoreError::other(format!(
                "CodeIsolationFailed: create invocation profile HRESULT={status:#x}"
            )));
        }
        Ok(Self {
            name,
            sid,
            deleted: false,
        })
    }

    pub(super) fn cleanup(&mut self) -> Result<()> {
        if !self.deleted {
            // SAFETY: only the unique profile successfully created by us.
            let status = unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
            if status < 0 {
                return Err(CoreError::other(format!(
                    "CodeIsolationFailed: delete invocation profile HRESULT={status:#x}"
                )));
            }
            self.deleted = true;
        }
        Ok(())
    }

    pub(super) fn folder(&self) -> Result<String> {
        let sid = wide(sid_string(self.sid)?)?;
        let mut output = null_mut();
        let status = unsafe { GetAppContainerFolderPath(sid.as_ptr(), &mut output) };
        if status < 0 {
            return Err(CoreError::other(format!(
                "CodeIsolationFailed: profile folder HRESULT={status:#x}"
            )));
        }
        // SAFETY: documented terminated UTF-16 output with CoTaskMem ownership.
        unsafe {
            let mut length = 0;
            while *output.add(length) != 0 {
                length += 1;
            }
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(output, length));
            CoTaskMemFree(output.cast());
            Ok(path)
        }
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            tracing::error!(%error, "code profile cleanup failed");
        }
        // SAFETY: CreateAppContainerProfile allocated this SID with FreeSid ownership.
        unsafe { FreeSid(self.sid) };
    }
}

pub(super) struct LocalAllocation(pub(super) *mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

pub(super) fn sid_string(sid: PSID) -> Result<String> {
    let mut output = null_mut();
    checked(
        unsafe { ConvertSidToStringSidW(sid, &mut output) },
        "format SID",
    )?;
    let _allocation = LocalAllocation(output.cast());
    let mut length = 0;
    // SAFETY: a successfully returned SID string is terminated and stays alive.
    unsafe {
        while *output.add(length) != 0 {
            length += 1;
        }
        Ok(String::from_utf16_lossy(std::slice::from_raw_parts(
            output, length,
        )))
    }
}

pub(super) fn grant_staging_readonly(path: &Path, package: PSID) -> Result<()> {
    let path = wide(path)?;
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(CoreError::other(format!(
            "CodeIsolationFailed: staging owner Win32={status}"
        )));
    }
    let _old = LocalAllocation(descriptor);
    // A protected DACL grants only the existing owner + SYSTEM full control,
    // and this one invocation SID read/execute. No ambient package grant.
    let sddl = wide(format!(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{})(A;OICI;GRGX;;;{})",
        sid_string(owner)?,
        sid_string(package)?
    ))?;
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
        "build staging DACL",
    )?;
    let _new = LocalAllocation(descriptor);
    let (mut present, mut defaulted, mut acl) = (0, 0, null_mut());
    checked(
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) },
        "read staging DACL",
    )?;
    if present == 0 || acl.is_null() {
        return Err(CoreError::other(
            "CodeIsolationFailed: missing staging DACL",
        ));
    }
    let status = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    };
    if status != 0 {
        return Err(CoreError::other(format!(
            "CodeIsolationFailed: apply staging DACL Win32={status}"
        )));
    }
    Ok(())
}
