//! Stable parent and fresh-file publication for the existing mutation boundary.

use super::*;
use deepagent_persistence::fs_commit::access::WindowsDacl;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use windows_sys::Wdk::Storage::FileSystem::{
    FileRenameInformation, NtSetInformationFile, FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_OPEN_IF,
    FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
    FILE_GENERIC_WRITE, READ_CONTROL, WRITE_DAC,
};

pub(in crate::fs_guard) struct WriteDirectory {
    // Children stay pinned until all operations and handle-based cleanup finish.
    guards: Vec<File>,
    pins: Vec<File>,
}

impl WriteDirectory {
    pub(in crate::fs_guard) fn open(parent: &Path, create: bool) -> io::Result<Self> {
        Self::open_inner(parent, create, true)
    }

    /// Read authorization pins names without writing guard files. The final
    /// content open still uses OBJ_DONT_REPARSE if an empty pin is converted.
    pub(in crate::fs_guard) fn pin(parent: &Path) -> io::Result<Self> {
        Self::open_inner(parent, false, false)
    }

    fn open_inner(parent: &Path, create: bool, protect_empty: bool) -> io::Result<Self> {
        // Validate the whole path before opening/creating any components.
        let (volume_root, _) = namespace_parts(parent)?;
        let mut components = parent.components();
        components.next();
        if components.next() != Some(Component::RootDir) {
            return Err(invalid("write parent must be absolute"));
        }
        let first = open_namespace_root(
            &volume_root,
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?;
        let mut directory = Self {
            guards: Vec::new(),
            pins: vec![first],
        };
        for component in components {
            let Component::Normal(name) = component else {
                return Err(invalid("write parent contains relative components"));
            };
            let mut name = relative_name(name)?;
            let child = open_name(
                directory.handle(),
                &mut name,
                FILE_TRAVERSE | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_OPEN,
                FILE_DIRECTORY_FILE,
            );
            let child = match child {
                Ok(child) => child,
                Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                    // Keep an empty existing parent nonempty before creating a
                    // child; no pathname-based mkdir can race these handles.
                    directory.guard(parent)?;
                    directory.validate()?;
                    open_name(
                        directory.handle(),
                        &mut name,
                        FILE_TRAVERSE | FILE_READ_ATTRIBUTES,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        FILE_OPEN_IF,
                        FILE_DIRECTORY_FILE,
                    )?
                }
                Err(error) => return Err(error),
            };
            directory.pins.push(child);
            directory.validate()?;
        }
        if protect_empty {
            directory.guard(parent)?;
        }
        directory.validate()?;
        Ok(directory)
    }

    fn handle(&self) -> HANDLE {
        self.pins.last().expect("volume pin exists").as_raw_handle()
    }

    pub(in crate::fs_guard) fn validate(&self) -> io::Result<()> {
        for pin in &self.pins {
            let metadata = pin.metadata()?;
            if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "write parent changed into a reparse point",
                ));
            }
        }
        Ok(())
    }

    pub(in crate::fs_guard) fn file(&self) -> &File {
        self.pins.last().expect("volume pin exists")
    }

    fn guard(&mut self, display_parent: &Path) -> io::Result<()> {
        let guard = tempfile::Builder::new()
            .prefix(".deepagent-directory-")
            .disable_cleanup(true)
            .make_in(display_parent, |generated| {
                open_name(
                    self.handle(),
                    &mut relative_name(
                        generated
                            .file_name()
                            .ok_or_else(|| invalid("missing guard name"))?,
                    )?,
                    FILE_GENERIC_READ | DELETE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    FILE_CREATE,
                    FILE_NON_DIRECTORY_FILE | FILE_DELETE_ON_CLOSE,
                )
            })?
            .into_parts()
            .0;
        self.guards.push(guard);
        Ok(())
    }

    pub(in crate::fs_guard) fn delete_entry(
        &self,
        path: &Path,
        recursive: bool,
    ) -> io::Result<bool> {
        let file = open_name(
            self.handle(),
            &mut relative_name(
                path.file_name()
                    .ok_or_else(|| invalid("missing entry name"))?,
            )?,
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT,
        )?;
        let metadata = file.metadata()?;
        let is_directory = metadata.is_dir();
        if is_directory && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            if !recursive {
                return Err(invalid(
                    "target is a directory; pass recursive=true to delete it",
                ));
            }
            drop(file);
            // Parent path is pinned; std 1.96 traverses children relative to
            // opened directory handles and never follows final reparse entries.
            std::fs::remove_dir_all(path)?;
        } else {
            mark_delete(&file)?;
        }
        Ok(is_directory)
    }

    fn access_policy(&self, name: &OsStr) -> io::Result<Option<WindowsDacl>> {
        match open_name(
            self.handle(),
            &mut relative_name(name)?,
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT,
        ) {
            Ok(file) => WindowsDacl::capture(&file).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(in crate::fs_guard) fn stage(
        &self,
        display_parent: &Path,
        name: &OsStr,
    ) -> io::Result<Stage> {
        let policy = self.access_policy(name)?;
        Ok(tempfile::Builder::new()
            .prefix(".deepagent-write-")
            .disable_cleanup(true)
            .make_in(display_parent, |generated| {
                let stage = Stage {
                    file: open_name(
                        self.handle(),
                        &mut relative_name(
                            generated
                                .file_name()
                                .ok_or_else(|| invalid("missing staging name"))?,
                        )?,
                        FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | WRITE_DAC,
                        FILE_SHARE_READ,
                        FILE_CREATE,
                        FILE_NON_DIRECTORY_FILE,
                    )?,
                    published: false,
                    policy: policy.clone(),
                };
                if let Some(policy) = &stage.policy {
                    policy.apply(&stage.file, true)?;
                }
                Ok(stage)
            })?
            .into_parts()
            .0)
    }
}

pub(in crate::fs_guard) struct Stage {
    pub(in crate::fs_guard) file: File,
    published: bool,
    policy: Option<WindowsDacl>,
}

impl Stage {
    pub(in crate::fs_guard) fn publish(
        mut self,
        parent: &WriteDirectory,
        name: &OsStr,
        replace: bool,
    ) -> io::Result<()> {
        if parent.access_policy(name)? != self.policy {
            return Err(io::Error::other("FILE_ACCESS_POLICY_CHANGED: target policy changed during staging; no content was published"));
        }
        if let Some(policy) = &self.policy {
            policy.apply(&self.file, false)?;
            if WindowsDacl::capture(&self.file)? != *policy {
                return Err(io::Error::other(
                    "file access policy could not be preserved before publication",
                ));
            }
        }
        let name = relative_name(name)?;
        let size = size_of::<FILE_RENAME_INFORMATION>() + size_of_val(name.as_slice());
        let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        let mut status_block = IO_STATUS_BLOCK {
            Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
            Information: 0,
        };
        // SAFETY: Aligned buffer holds the fixed header and full UTF-16 tail;
        // source and destination directory handles live throughout this call.
        let status = unsafe {
            (*info).Anonymous.ReplaceIfExists = u8::from(replace);
            (*info).RootDirectory = parent.handle();
            (*info).FileNameLength = size_of_val(name.as_slice()) as u32;
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                buffer
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(offset_of!(FILE_RENAME_INFORMATION, FileName))
                    .cast::<u16>(),
                name.len(),
            );
            NtSetInformationFile(
                self.file.as_raw_handle(),
                &mut status_block,
                info.cast(),
                size as u32,
                FileRenameInformation,
            )
        };
        if status < 0 {
            // SAFETY: Pointer-free NTSTATUS conversion.
            return Err(io::Error::from_raw_os_error(unsafe {
                RtlNtStatusToDosError(status) as i32
            }));
        }
        self.published = true;
        Ok(())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        if let Err(error) = mark_delete(&self.file) {
            tracing::warn!(error = %error, "failed to remove owned write staging file");
        }
    }
}

fn mark_delete(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    // SAFETY: The owned entry handle and fixed, initialized buffer stay alive
    // for the synchronous call. No mutable pathname is resolved for deletion.
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            ptr::addr_of!(disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn relative_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let name: Vec<_> = name.encode_wide().collect();
    if name.is_empty()
        || name
            .iter()
            .any(|unit| [0, b':' as u16, b'\\' as u16, b'/' as u16].contains(unit))
        || name == [b'.' as u16]
        || name == [b'.' as u16, b'.' as u16]
    {
        return Err(invalid("invalid handle-relative name"));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_guard::windows_tests::{
        junction_data, junction_write_handle, set_junction_data, Junction,
    };

    #[test]
    fn write_target_guard_installation_after_parent_conversion_does_not_touch_outside() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let template_path = workspace.path().join("template");
        let template = Junction::new(&template_path, outside.path());
        let bytes = junction_data(&template_path);
        let parent = workspace.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        // Construct the same native parent pin just before guard installation;
        // no production callback or alternative mutation path is introduced.
        let mut directory = WriteDirectory::pin(&parent).unwrap();
        let writer = junction_write_handle(&parent);
        set_junction_data(&writer, &bytes).unwrap();
        let guard = directory.guard(&parent);
        eprintln!(
            "guard after parent conversion: ok={}, error={:?}",
            guard.is_ok(),
            guard.as_ref().err()
        );
        assert!(
            directory.validate().is_err(),
            "converted parent must never authorize a write"
        );
        assert_eq!(
            std::fs::read_dir(outside.path()).unwrap().count(),
            0,
            "handle-relative guard must not land outside, even before validation"
        );
        drop(directory);
        drop(writer);
        std::fs::remove_dir(&parent).unwrap();
        drop(template);
    }
}
