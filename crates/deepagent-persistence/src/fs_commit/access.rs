//! Versioned Windows DACL snapshots for file publication and checkpoints.
//!
//! Only the DACL and its inheritance protection are included, not owner/SACL.
//! Policy material contains local identities: never put it in logs or Debug.

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsDacl {
    version: u32,
    protected: bool,
    // None is a NULL DACL (full access), never an empty/deny-all DACL.
    sddl: Option<String>,
}

impl std::fmt::Debug for WindowsDacl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsDacl")
            .field("version", &self.version)
            .field("protected", &self.protected)
            .finish_non_exhaustive()
    }
}

#[cfg(windows)]
mod native {
    use super::WindowsDacl;
    use std::fs::File;
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SetSecurityInfo,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, READ_CONTROL, WRITE_DAC,
    };
    use windows_sys::Win32::System::Memory::LocalSize;

    // Local serialization budget, not an ABI limit. Native ACLs are <=64KiB;
    // text encoding is larger. Reject corrupt/unbounded checkpoint metadata.
    const MAX_SDDL_UNITS: usize = 1024 * 1024;

    struct LocalBuffer(*mut std::ffi::c_void);

    impl Drop for LocalBuffer {
        fn drop(&mut self) {
            // SAFETY: All buffers are allocated by the security APIs below,
            // documented to be freed with LocalFree, and owned exactly once.
            unsafe { LocalFree(self.0) };
        }
    }

    fn invalid() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "invalid Windows DACL snapshot")
    }

    /// # Safety
    /// `text` must point to the native conversion's allocation of `capacity`
    /// WCHARs, initialized through its first NUL. Never read unused capacity.
    unsafe fn decode_native_text(text: *const u16, capacity: usize) -> io::Result<String> {
        if text.is_null() || capacity == 0 || capacity > MAX_SDDL_UNITS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid native DACL buffer: null={}, capacity={capacity}",
                    text.is_null()
                ),
            ));
        }
        let mut length = 0;
        // SAFETY: The caller supplies the live native buffer; scan only up to
        // the first initialized terminator, bounded by the allocation size.
        while length < capacity && unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        if length == capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native DACL buffer has no terminator within its capacity",
            ));
        }
        // SAFETY: The scan established that these units precede the NUL.
        let units = unsafe { std::slice::from_raw_parts(text, length) };
        String::from_utf16(units).map_err(|_| invalid())
    }

    fn regular_entry(file: File) -> io::Result<File> {
        let metadata = file.metadata()?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !(metadata.is_dir() || metadata.is_file())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "DACL snapshot requires a regular file or directory",
            ));
        }
        Ok(file)
    }

    /// Open an existing snapshot source without following its final reparse
    /// entry. File sharing excludes concurrent writes through ordinary handles.
    /// Ancestor authorization remains the caller's responsibility.
    pub fn open_snapshot_source(path: &Path) -> io::Result<File> {
        let directory = std::fs::symlink_metadata(path)?.is_dir();
        regular_entry(
            std::fs::OpenOptions::new()
                .access_mode(FILE_GENERIC_READ | READ_CONTROL)
                .share_mode(if directory {
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
                } else {
                    FILE_SHARE_READ
                })
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?,
        )
    }

    /// Create an empty staging entry and retain the rights required to write
    /// its content and DACL even after applying the source's restrictive policy.
    /// No sensitive bytes may be written before the caller protects the DACL.
    pub fn create_snapshot_target(path: &Path, directory: bool) -> io::Result<File> {
        if directory {
            std::fs::create_dir(path)?;
        }
        regular_entry(
            std::fs::OpenOptions::new()
                .write(!directory)
                .access_mode(if directory {
                    READ_CONTROL | WRITE_DAC | FILE_WRITE_ATTRIBUTES
                } else {
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC
                })
                .create_new(!directory)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?,
        )
    }

    /// Read policy after publication, without requesting content/write rights.
    pub fn open_policy_source(path: &Path) -> io::Result<File> {
        regular_entry(
            std::fs::OpenOptions::new()
                .access_mode(READ_CONTROL)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?,
        )
    }

    impl WindowsDacl {
        /// Read policy from an already authorized, stable READ_CONTROL handle.
        pub fn capture(file: &File) -> io::Result<Self> {
            let mut descriptor = ptr::null_mut();
            let mut dacl = ptr::null_mut();
            // SAFETY: Output pointers are initialized; file remains alive.
            let code = unsafe {
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut dacl,
                    ptr::null_mut(),
                    &mut descriptor,
                )
            };
            let descriptor = LocalBuffer(descriptor);
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
            let mut control = 0;
            let mut revision = 0;
            // SAFETY: GetSecurityInfo returned a valid descriptor.
            if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) }
                == 0
            {
                return Err(io::Error::last_os_error());
            }
            let sddl = if dacl.is_null() {
                None
            } else {
                let mut text = ptr::null_mut();
                // SAFETY: Descriptor is valid and output allocations are
                // released on every branch. Request DACL only, no owner/SACL.
                let ok = unsafe {
                    ConvertSecurityDescriptorToStringSecurityDescriptorW(
                        descriptor.0,
                        SDDL_REVISION_1,
                        DACL_SECURITY_INFORMATION,
                        &mut text,
                        ptr::null_mut(),
                    )
                };
                let text = LocalBuffer(text.cast());
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                // The optional conversion size is not wcslen: alias expansion
                // can overallocate; empty ACLs also expose terminator-count
                // differences on the tested OS. Bound by the actual local
                // allocation and stop at its first initialized NUL instead.
                // SAFETY: Conversion returns a live LocalFree allocation.
                let bytes = unsafe { LocalSize(text.0) };
                if bytes == 0 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: The successful converter guarantees a terminated
                // string in this allocation; decode never reads unused tail.
                Some(unsafe { decode_native_text(text.0.cast(), bytes / size_of::<u16>()) }?)
            };
            Ok(Self {
                version: 1,
                protected: control & SE_DACL_PROTECTED != 0,
                sddl,
            })
        }

        /// Apply before writing any sensitive bytes. Staging is protected from
        /// parent inheritance; publication restores the original protection.
        /// The caller must retain its WRITE_DAC handle through both operations.
        pub fn apply(&self, file: &File, staging: bool) -> io::Result<()> {
            let descriptor = self.validated_descriptor()?;
            let mut dacl = ptr::null_mut();
            if let Some(descriptor) = &descriptor {
                let mut present = 0;
                let mut defaulted = 0;
                // SAFETY: validated_descriptor checked this native descriptor.
                if unsafe {
                    GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
            }
            let protection = if staging || self.protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
            // SAFETY: dacl is NULL for an explicitly captured NULL policy or
            // points inside the live parsed descriptor. Only DACL is changed.
            let code = unsafe {
                SetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | protection,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null(),
                )
            };
            if code == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(code as i32))
            }
        }

        /// Validate untrusted persisted metadata without changing any object.
        pub fn validate(&self) -> io::Result<()> {
            self.validated_descriptor().map(|_| ())
        }

        fn validated_descriptor(&self) -> io::Result<Option<LocalBuffer>> {
            if self.version != 1 {
                return Err(invalid());
            }
            let descriptor = self.parse()?;
            let mut dacl: *mut ACL = ptr::null_mut();
            if let Some(descriptor) = &descriptor {
                let mut present = 0;
                let mut defaulted = 0;
                let mut control = 0;
                let mut revision = 0;
                // SAFETY: The native SDDL parser returned a valid descriptor.
                if unsafe {
                    GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
                } == 0
                    || unsafe {
                        GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision)
                    } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if present == 0
                    || dacl.is_null()
                    || (control & SE_DACL_PROTECTED != 0) != self.protected
                {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, format!(
                        "inconsistent Windows DACL snapshot: present={}, null={}, captured_protected={}, parsed_protected={}",
                        present != 0, dacl.is_null(), self.protected, control & SE_DACL_PROTECTED != 0
                    )));
                }
            }
            Ok(descriptor)
        }

        fn parse(&self) -> io::Result<Option<LocalBuffer>> {
            let Some(text) = &self.sddl else {
                return Ok(None);
            };
            if !text.starts_with("D:") || text.contains('\0') || text.len() > MAX_SDDL_UNITS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                    "invalid Windows DACL text: dacl_prefix={}, embedded_nul={}, over_budget={}",
                    text.starts_with("D:"), text.contains('\0'), text.len() > MAX_SDDL_UNITS
                ),
                ));
            }
            let text = text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let mut descriptor = ptr::null_mut();
            // SAFETY: Bounded, NUL-terminated input; the native parser validates
            // the descriptor instead of interpreting untrusted binary offsets.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            };
            let descriptor = LocalBuffer(descriptor);
            if ok == 0 {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "native Windows DACL parser failed: {}",
                        io::Error::last_os_error()
                    ),
                ))
            } else {
                Ok(Some(descriptor))
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_GENERIC_READ, FILE_GENERIC_WRITE, WRITE_DAC,
        };

        #[test]
        fn inherited_policy_survives_protected_staging_and_publication() {
            let directory = tempfile::tempdir().unwrap();
            let original = File::create(directory.path().join("original")).unwrap();
            let staged = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | WRITE_DAC)
                .open(directory.path().join("staged"))
                .unwrap();
            let before = WindowsDacl::capture(&original).unwrap();
            assert!(!before.protected, "fixture must inherit its parent policy");
            before.apply(&staged, true).unwrap();
            before.apply(&staged, false).unwrap();
            let after = WindowsDacl::capture(&staged).unwrap();
            let shape = |policy: &WindowsDacl| {
                let text = policy.sddl.as_deref().unwrap_or_default();
                (
                    policy.protected,
                    text.split('(').next().unwrap_or_default().to_owned(),
                    text.matches('(').count(),
                )
            };
            assert!(
                before == after,
                "policy shape changed: {:?} -> {:?}",
                shape(&before),
                shape(&after)
            );
        }

        #[test]
        fn native_text_uses_terminator_not_allocation_capacity() {
            for units in [
                vec![68, 58, 0],
                vec![68, 58, 0, 99, 0],
                vec![68, 58, 0, 0xd800],
            ] {
                // SAFETY: The synthetic buffer is initialized and live.
                assert_eq!(
                    unsafe { decode_native_text(units.as_ptr(), units.len()) }.unwrap(),
                    "D:"
                );
            }
            for units in [vec![], vec![68, 58], vec![0xd800, 0]] {
                // SAFETY: The synthetic buffer is initialized and live.
                assert!(unsafe { decode_native_text(units.as_ptr(), units.len()) }.is_err());
            }
        }

        #[test]
        fn null_and_empty_dacl_remain_distinct_through_staging() {
            for sddl in [None, Some("D:P")] {
                let directory = tempfile::tempdir().unwrap();
                let file = create_snapshot_target(&directory.path().join("entry"), false).unwrap();
                let original = WindowsDacl::capture(&file).unwrap();
                let policy = WindowsDacl {
                    version: 1,
                    protected: true,
                    sddl: sddl.map(str::to_owned),
                };
                policy.apply(&file, true).unwrap();
                let staged = WindowsDacl::capture(&file).unwrap_or_else(|error| {
                    panic!(
                        "capture null/empty policy failed: null={}, error={error}",
                        sddl.is_none()
                    )
                });
                assert!(staged.protected);
                assert_eq!(staged.sddl.is_none(), sddl.is_none());
                assert!(!staged.sddl.as_deref().unwrap_or_default().contains('('));
                policy.apply(&file, false).unwrap();
                assert!(WindowsDacl::capture(&file).unwrap() == staged);
                original.apply(&file, false).unwrap();
            }
        }

        #[test]
        fn inherited_policy_roundtrips_and_debug_does_not_expose_identities() {
            let file = tempfile::tempfile().unwrap();
            let policy = WindowsDacl::capture(&file).unwrap();
            let wire = serde_json::to_string(&policy).unwrap();
            let restored: WindowsDacl = serde_json::from_str(&wire).unwrap();
            assert_eq!(restored, policy);
            assert!(!format!("{policy:?}").contains("sddl"));
            if let Some(text) = &policy.sddl {
                assert!(!format!("{policy:?}").contains(text));
            }
        }

        #[test]
        fn malformed_or_future_metadata_is_rejected_without_changing_target() {
            let file = tempfile::tempfile().unwrap();
            let before = WindowsDacl::capture(&file).unwrap();
            for (version, sddl) in [(2, None), (1, Some("bad")), (1, Some("D:\0(A;;FA;;;SY)"))] {
                let policy = WindowsDacl {
                    version,
                    protected: true,
                    sddl: sddl.map(str::to_owned),
                };
                assert!(policy.apply(&file, false).is_err());
                assert_eq!(WindowsDacl::capture(&file).unwrap(), before);
            }
        }
    }
}

#[cfg(windows)]
pub use native::{create_snapshot_target, open_policy_source, open_snapshot_source};
