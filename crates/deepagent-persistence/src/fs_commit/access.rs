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
        ConvertToAutoInheritPrivateObjectSecurity, CreatePrivateObjectSecurityEx,
        DestroyPrivateObjectSecurity, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetSecurityDescriptorGroup, GetSecurityDescriptorOwner, InitializeSecurityDescriptor,
        SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorGroup,
        SetSecurityDescriptorOwner, ACL, DACL_SECURITY_INFORMATION, GENERIC_MAPPING,
        GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_DESCRIPTOR, SEF_AVOID_OWNER_CHECK,
        SEF_AVOID_PRIVILEGE_CHECK, SEF_DACL_AUTO_INHERIT, SE_DACL_AUTO_INHERITED,
        SE_DACL_PROTECTED, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_WRITE_ATTRIBUTES, READ_CONTROL, WRITE_DAC,
    };
    use windows_sys::Win32::System::Memory::LocalSize;

    // Local serialization budget, not an ABI limit. Native ACLs are <=64KiB;
    // text encoding is larger. Reject corrupt/unbounded checkpoint metadata.
    const MAX_SDDL_UNITS: usize = 1024 * 1024;

    struct LocalBuffer(*mut std::ffi::c_void);

    struct PrivateDescriptor(*mut std::ffi::c_void);

    impl Drop for PrivateDescriptor {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: Owned allocation returned by the private-security API.
                unsafe { DestroyPrivateObjectSecurity(&self.0) };
            }
        }
    }

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
                    READ_CONTROL | WRITE_DAC | FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES
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
                .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?,
        )
    }

    impl WindowsDacl {
        /// Read policy from a stable READ_CONTROL | FILE_READ_ATTRIBUTES handle.
        pub fn capture(file: &File) -> io::Result<Self> {
            let descriptor = read_descriptor(file, DACL_SECURITY_INFORMATION)?;
            // SAFETY: The native query returned a live, validated descriptor.
            unsafe { Self::from_descriptor(descriptor.0) }
        }

        /// # Safety
        /// `descriptor` must be a live, validated native security descriptor.
        unsafe fn from_descriptor(descriptor: *mut std::ffi::c_void) -> io::Result<Self> {
            let mut dacl = ptr::null_mut();
            let mut present = 0;
            let mut defaulted = 0;
            // SAFETY: The caller guarantees the native descriptor's validity.
            if unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut control = 0;
            let mut revision = 0;
            // SAFETY: The caller supplied a valid native descriptor.
            if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
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
                        descriptor,
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
        /// parent inheritance; publication restores the original protection,
        /// and Windows may propagate inherited rules to existing descendants.
        /// Use `publish` to prove the current parent before removing protection.
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

        /// Verify a publication without changing persisted snapshot identity.
        /// Only the authorized D: -> D:AI transition is accepted, with identical
        /// ACE text/order/flags, protection and a native proof of parent rules.
        pub fn verify_publication(
            &self,
            file: &File,
            parent: impl FnOnce() -> io::Result<File>,
        ) -> io::Result<bool> {
            self.validate()?;
            let actual = Self::capture(file)?;
            if !self.matches_publication_shape(&actual) {
                return Ok(false);
            }
            Ok(self.expected_publication(file, parent)?.as_ref() == Some(&actual))
        }

        /// Prove inheritance before exposing staging to its current parent,
        /// apply through the retained handle, then strictly verify the result.
        /// On a post-application failure, restore the protected staging policy.
        pub fn publish(
            &self,
            file: &File,
            parent: impl FnOnce() -> io::Result<File>,
        ) -> io::Result<()> {
            let expected = self.expected_publication(file, parent)?.ok_or_else(|| {
                io::Error::other(
                    "Windows DACL differs after publication: parent cannot preserve snapshot",
                )
            })?;
            let result = expected.apply(file, false).and_then(|()| {
                if Self::capture(file)? != expected {
                    return Err(io::Error::other("Windows DACL differs after publication"));
                }
                Ok(())
            });
            if let Err(error) = result {
                self.apply(file, true).map_err(|rollback| {
                    io::Error::other(format!(
                        "{error}; staging DACL protection recovery failed: {rollback}"
                    ))
                })?;
                return Err(error);
            }
            if expected != *self {
                tracing::info!(
                    legacy_inheritance_upgraded = true,
                    "Windows DACL publication verified a native inheritance-model upgrade"
                );
            }
            Ok(())
        }

        fn matches_publication_shape(&self, actual: &Self) -> bool {
            if self.protected || self.sddl.is_none() {
                return actual == self;
            }
            // Never normalize snapshot equality or policy-drift comparisons.
            let rules = self
                .sddl
                .as_deref()
                .and_then(|text| {
                    text.strip_prefix("D:AI")
                        .or_else(|| text.strip_prefix("D:"))
                })
                .filter(|rules| rules.starts_with('('));
            !actual.protected
                && rules.is_some()
                && actual
                    .sddl
                    .as_deref()
                    .and_then(|text| text.strip_prefix("D:AI"))
                    == rules
        }

        fn expected_publication(
            &self,
            file: &File,
            parent: impl FnOnce() -> io::Result<File>,
        ) -> io::Result<Option<Self>> {
            self.validate()?;
            if self.protected || self.sddl.is_none() {
                return Ok(Some(self.clone()));
            }
            let flags =
                DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION;
            let parent = read_descriptor(&parent()?, flags)?;
            let original = self.validated_descriptor()?.ok_or_else(invalid)?;
            let legacy = !self.sddl.as_deref().unwrap_or_default().starts_with("D:AI");
            let converted = publication_descriptor(file, &original, &parent, legacy)?;
            // SAFETY: Successful native conversion returned a live descriptor.
            let expected = unsafe { Self::from_descriptor(converted.0) }?;
            if !self.matches_publication_shape(&expected) {
                return Ok(None);
            }
            if !legacy {
                // AI snapshots already distinguish explicit/inherited ACEs.
                // Reclassifying them can incorrectly protect explicit-only ACLs.
                return Ok(Some(expected));
            }
            // Conversion only classifies the *old* ACEs. Recompute inheritance
            // too, so an additional or partially changed parent ACE cannot be
            // discovered only after the stage has already been unprotected.
            // SAFETY: Both native descriptors remain live through the call.
            let inherited =
                unsafe { inherit_descriptor(parent.0, converted.0, file.metadata()?.is_dir()) }?;
            // SAFETY: Successful native inheritance returned a live descriptor.
            if unsafe { Self::from_descriptor(inherited.0) }? != expected {
                return Ok(None);
            }
            Ok(Some(expected))
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

    fn file_mapping() -> GENERIC_MAPPING {
        GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        }
    }

    fn publication_descriptor(
        file: &File,
        original: &LocalBuffer,
        parent: &LocalBuffer,
        legacy: bool,
    ) -> io::Result<PrivateDescriptor> {
        // The converter needs owner/group. Read them only into memory; snapshot
        // serialization and filesystem writes continue to include DACL only.
        let identities = read_descriptor(
            file,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION,
        )?;
        let mut owner = ptr::null_mut();
        let mut group = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let mut present = 0;
        let mut defaulted = 0;
        // SAFETY: Plain native descriptor is initialized before any use.
        let mut proposed: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
        let proposed_ptr = ptr::addr_of_mut!(proposed).cast();
        // SAFETY: All allocations remain live; the absolute descriptor borrows
        // their owner/group/DACL only until conversion finishes.
        if unsafe { GetSecurityDescriptorOwner(identities.0, &mut owner, &mut defaulted) } == 0
            || unsafe { GetSecurityDescriptorGroup(identities.0, &mut group, &mut defaulted) } == 0
            || unsafe {
                GetSecurityDescriptorDacl(original.0, &mut present, &mut dacl, &mut defaulted)
            } == 0
            || unsafe { InitializeSecurityDescriptor(proposed_ptr, 1) } == 0
            || unsafe { SetSecurityDescriptorOwner(proposed_ptr, owner, 0) } == 0
            || unsafe { SetSecurityDescriptorGroup(proposed_ptr, group, 0) } == 0
            || unsafe { SetSecurityDescriptorDacl(proposed_ptr, 1, dacl, 0) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let directory = file.metadata()?.is_dir();
        if !legacy {
            // Preserve known explicit/inherited flags instead of reclassifying.
            // SAFETY: proposed_ptr is the live initialized absolute descriptor.
            if unsafe {
                SetSecurityDescriptorControl(
                    proposed_ptr,
                    SE_DACL_AUTO_INHERITED,
                    SE_DACL_AUTO_INHERITED,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: Parent and borrowed owner/group/DACL remain live.
            return unsafe { inherit_descriptor(parent.0, proposed_ptr, directory) };
        }
        let mut converted = ptr::null_mut();
        // SAFETY: Native conversion has no filesystem side effects.
        let ok = unsafe {
            ConvertToAutoInheritPrivateObjectSecurity(
                parent.0,
                proposed_ptr,
                &mut converted,
                ptr::null(),
                u8::from(directory),
                &file_mapping(),
            )
        };
        let converted = PrivateDescriptor(converted);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(converted)
    }

    /// # Safety
    /// Parent and creator must point to live, valid native descriptors.
    unsafe fn inherit_descriptor(
        parent: *mut std::ffi::c_void,
        creator: *mut std::ffi::c_void,
        directory: bool,
    ) -> io::Result<PrivateDescriptor> {
        let mut inherited = ptr::null_mut();
        // SAFETY: Caller owns the input descriptors. This computes a proof in
        // memory only; it neither sets owner/SACL nor grants filesystem access.
        let ok = unsafe {
            CreatePrivateObjectSecurityEx(
                parent,
                creator,
                &mut inherited,
                ptr::null(),
                i32::from(directory),
                SEF_DACL_AUTO_INHERIT | SEF_AVOID_OWNER_CHECK | SEF_AVOID_PRIVILEGE_CHECK,
                ptr::null_mut(),
                &file_mapping(),
            )
        };
        let inherited = PrivateDescriptor(inherited);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(inherited)
    }

    fn read_descriptor(file: &File, information: u32) -> io::Result<LocalBuffer> {
        // GetSecurityInfo can synthesize a protected, non-inherited view when
        // this metadata right is absent. Never capture that as the file policy.
        use windows_sys::Win32::Storage::FileSystem::{
            FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_TAG_INFO,
        };
        let mut attributes = std::mem::MaybeUninit::<FILE_ATTRIBUTE_TAG_INFO>::uninit();
        // SAFETY: Correctly sized, aligned output and a live authorized handle.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileAttributeTagInfo,
                attributes.as_mut_ptr().cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut descriptor = ptr::null_mut();
        // SAFETY: Output initialized, allocation owned once and freed locally.
        let code = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                information,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        let descriptor = LocalBuffer(descriptor);
        if code != 0 {
            Err(io::Error::from_raw_os_error(code as i32))
        } else {
            Ok(descriptor)
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
            before
                .publish(&staged, || open_policy_source(directory.path()))
                .unwrap();
            let after = WindowsDacl::capture(&staged).unwrap();
            let shape = |policy: &WindowsDacl| {
                let text = policy.sddl.as_deref().unwrap_or_default();
                (
                    policy.protected,
                    text.split('(').next().unwrap_or_default().to_owned(),
                    text.split('(')
                        .skip(1)
                        .map(|ace| ace.split(';').take(3).collect::<Vec<_>>().join(";"))
                        .collect::<Vec<_>>(),
                )
            };
            assert!(
                before
                    .verify_publication(&staged, || open_policy_source(directory.path()))
                    .unwrap(),
                "policy shape changed: {:?} -> {:?}",
                shape(&before),
                shape(&after)
            );
        }

        #[test]
        fn legacy_upgrade_requires_native_proof_and_preserves_real_access() {
            use std::io::Write;
            use windows_sys::Win32::Storage::FileSystem::FILE_EXECUTE;
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("controlled-parent");
            let parent = create_snapshot_target(&root, true).unwrap();
            let restricted = WindowsDacl {
                version: 1,
                protected: true,
                sddl: Some("D:P(D;OICI;0x20;;;WD)(A;OICI;FA;;;WD)".into()),
            };
            restricted.apply(&parent, false).unwrap();
            let source_path = root.join("source");
            let source = create_snapshot_target(&source_path, false).unwrap();
            let actual_source = WindowsDacl::capture(&source).unwrap();
            let rules = &actual_source.sddl.as_ref().unwrap()
                [actual_source.sddl.as_ref().unwrap().find('(').unwrap()..];
            for prefix in ["D:", "D:AI"] {
                // Version-1 legacy metadata replay, independent of host ACLs.
                let policy = WindowsDacl {
                    version: 1,
                    protected: false,
                    sddl: Some(format!("{prefix}{rules}")),
                };
                let wire = serde_json::to_string(&policy).unwrap();
                let policy: WindowsDacl = serde_json::from_str(&wire).unwrap();
                let staged_path = root.join(if prefix == "D:" { "legacy" } else { "auto" });
                let mut staged = create_snapshot_target(&staged_path, false).unwrap();
                policy.apply(&staged, true).unwrap();
                assert!(WindowsDacl::capture(&staged).unwrap().protected);
                staged.write_all(b"synthetic body").unwrap();
                policy
                    .publish(&staged, || open_policy_source(&root))
                    .unwrap();
                assert!(policy
                    .verify_publication(&staged, || open_policy_source(&root))
                    .unwrap());
                let after = WindowsDacl::capture(&staged).unwrap();
                assert!(!after.protected);
                assert_eq!(after.sddl.as_deref(), Some(format!("D:AI{rules}").as_str()));
                assert_eq!(std::fs::read(&staged_path).unwrap(), b"synthetic body");
                for path in [&source_path, &staged_path] {
                    let denied = std::fs::OpenOptions::new()
                        .access_mode(FILE_EXECUTE)
                        .open(path)
                        .unwrap_err();
                    assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
                    assert!(File::open(path).is_ok(), "read must remain allowed");
                }
                assert_eq!(
                    serde_json::to_string(&policy).unwrap(),
                    wire,
                    "verification mutated the persisted snapshot"
                );
                for replacement in [
                    rules.replacen("WP", "0x2", 1),
                    rules.replacen(";ID;", ";;", 1),
                    rules.replacen("(D;", "(A;", 1),
                ] {
                    assert_ne!(replacement, rules, "negative fixture must differ");
                    let wrong = WindowsDacl {
                        sddl: Some(format!("D:{replacement}")),
                        ..policy.clone()
                    };
                    assert!(!wrong
                        .verify_publication(&staged, || panic!(
                            "different ACEs must reject without parent I/O"
                        ))
                        .unwrap());
                }
            }
        }

        #[test]
        fn legacy_upgrade_rejects_unrelated_parent_and_never_ignores_protection() {
            let directory = tempfile::tempdir().unwrap();
            let original =
                create_snapshot_target(&directory.path().join("original"), false).unwrap();
            let captured = WindowsDacl::capture(&original).unwrap();
            let text = captured.sddl.as_ref().unwrap();
            let rules = &text[text.find('(').unwrap()..];
            let policy = WindowsDacl {
                version: 1,
                protected: false,
                sddl: Some(format!("D:{rules}")),
            };
            let staged = create_snapshot_target(&directory.path().join("staged"), false).unwrap();
            policy.apply(&staged, true).unwrap();
            assert!(!policy
                .verify_publication(&staged, || panic!("protected output must reject"))
                .unwrap());
            policy.apply(&staged, false).unwrap();
            let unrelated = directory.path().join("unrelated");
            let parent = create_snapshot_target(&unrelated, true).unwrap();
            WindowsDacl {
                version: 1,
                protected: true,
                sddl: Some("D:P(A;;FA;;;WD)".into()),
            }
            .apply(&parent, false)
            .unwrap();
            assert!(
                !policy
                    .verify_publication(&staged, || open_policy_source(&unrelated))
                    .unwrap(),
                "prefix-only equivalence must not bypass native inheritance proof"
            );
            assert!(policy
                .verify_publication(&staged, || open_policy_source(directory.path()))
                .unwrap());
        }

        #[test]
        fn changed_parent_is_rejected_before_staging_is_unprotected() {
            use std::io::Write;
            use windows_sys::Win32::Storage::FileSystem::{FILE_EXECUTE, FILE_WRITE_DATA};
            for prefix in ["D:", "D:AI"] {
                for changed in [
                    "D:P(D;OICI;0x22;;;WD)(A;OICI;FA;;;WD)",
                    "D:P(D;OICI;0x20;;;WD)(D;OICI;0x2;;;WD)(A;OICI;FA;;;WD)",
                    "D:P(A;OICI;FA;;;WD)",
                ] {
                    let directory = tempfile::tempdir().unwrap();
                    let root = directory.path().join("controlled");
                    let parent = create_snapshot_target(&root, true).unwrap();
                    let initial = WindowsDacl {
                        version: 1,
                        protected: true,
                        sddl: Some("D:P(D;OICI;0x20;;;WD)(A;OICI;FA;;;WD)".into()),
                    };
                    initial.apply(&parent, false).unwrap();
                    let source = create_snapshot_target(&root.join("source"), false).unwrap();
                    let captured = WindowsDacl::capture(&source).unwrap();
                    let text = captured.sddl.unwrap();
                    let rules = &text[text.find('(').unwrap()..];
                    let policy = WindowsDacl {
                        version: 1,
                        protected: false,
                        sddl: Some(format!("{prefix}{rules}")),
                    };
                    let staged_path = root.join("staged");
                    let mut staged = create_snapshot_target(&staged_path, false).unwrap();
                    policy.apply(&staged, true).unwrap();
                    staged.write_all(b"protected synthetic content").unwrap();
                    let protected = WindowsDacl::capture(&staged).unwrap();
                    WindowsDacl {
                        sddl: Some(changed.into()),
                        ..initial
                    }
                    .apply(&parent, false)
                    .unwrap();
                    assert!(policy
                        .publish(&staged, || open_policy_source(&root))
                        .is_err());
                    assert_eq!(
                        WindowsDacl::capture(&staged).unwrap(),
                        protected,
                        "parent mismatch exposed a previously protected stage"
                    );
                    assert_eq!(
                        std::fs::read(&staged_path).unwrap(),
                        b"protected synthetic content"
                    );
                    assert_eq!(
                        std::fs::OpenOptions::new()
                            .access_mode(FILE_EXECUTE)
                            .open(&staged_path)
                            .unwrap_err()
                            .kind(),
                        io::ErrorKind::PermissionDenied
                    );
                    let changed_access = std::fs::OpenOptions::new()
                        .access_mode(if changed.contains("(D;") {
                            FILE_WRITE_DATA
                        } else {
                            FILE_EXECUTE
                        })
                        .open(root.join("source"));
                    if changed.contains("(D;") {
                        assert_eq!(
                            changed_access.unwrap_err().kind(),
                            io::ErrorKind::PermissionDenied
                        );
                    } else {
                        assert!(
                            changed_access.is_ok(),
                            "fixture must really remove the inherited execute denial"
                        );
                    }
                }
            }
        }

        #[test]
        fn auto_inherited_explicit_only_policy_remains_publishable() {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().join("controlled");
            let parent = create_snapshot_target(&root, true).unwrap();
            WindowsDacl {
                version: 1,
                protected: true,
                sddl: Some("D:P(A;;FA;;;WD)".into()),
            }
            .apply(&parent, false)
            .unwrap();
            let file = create_snapshot_target(&root.join("entry"), false).unwrap();
            let policy = WindowsDacl {
                version: 1,
                protected: false,
                sddl: Some("D:AI(D;;0x20;;;WD)(A;;FA;;;WD)".into()),
            };
            policy.apply(&file, false).unwrap();
            let policy = WindowsDacl::capture(&file).unwrap();
            assert!(!policy.protected);
            assert!(
                policy.sddl.as_deref().unwrap().starts_with("D:AI("),
                "fixture must have an unprotected AI policy"
            );
            assert!(
                !policy.sddl.as_deref().unwrap().contains(";ID;"),
                "fixture must not have inherited ACEs"
            );
            policy.apply(&file, true).unwrap();
            policy.publish(&file, || open_policy_source(&root)).unwrap();
            assert_eq!(WindowsDacl::capture(&file).unwrap(), policy);
            policy.apply(&file, true).unwrap();
            let protected = WindowsDacl::capture(&file).unwrap();
            WindowsDacl {
                version: 1,
                protected: true,
                sddl: Some("D:P(D;OICI;0x2;;;WD)(A;;FA;;;WD)".into()),
            }
            .apply(&parent, false)
            .unwrap();
            assert!(policy.publish(&file, || open_policy_source(&root)).is_err());
            assert_eq!(WindowsDacl::capture(&file).unwrap(), protected);
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
                // Native capture, not manually supplied seed text: Windows may
                // add AI even when applying a protected empty ACL.
                staged
                    .publish(&file, || {
                        panic!("protected policy must not query its parent")
                    })
                    .unwrap();
                assert!(WindowsDacl::capture(&file).unwrap() == staged);
                assert!(staged
                    .verify_publication(&file, || panic!("protected policy needs no parent proof"))
                    .unwrap());
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
