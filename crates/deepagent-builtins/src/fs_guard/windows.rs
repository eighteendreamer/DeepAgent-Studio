//! Windows operation-time opens. Protocol/permission policy stays in WorkspaceRoot.
//!
//! NtCreateFile's OBJ_DONT_REPARSE covers the entire name in one native open,
//! unlike CreateFile's final-component OPEN_REPARSE_POINT. Use the existing
//! windows-sys bindings rather than maintaining a second FFI layout/signature.

use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr;

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    NtCreateFile, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    RtlNtStatusToDosError, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
    STATUS_REPARSE_POINT_ENCOUNTERED, UNICODE_STRING,
};
use windows_sys::Win32::Security::{
    SecurityIdentification, SECURITY_QUALITY_OF_SERVICE, SECURITY_STATIC_TRACKING,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileFullDirectoryInfo, FileFullDirectoryRestartInfo, GetFileInformationByHandleEx, GetFileType,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO,
    FILE_GENERIC_READ, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_TYPE_DISK, SECURITY_IDENTIFICATION,
    SYNCHRONIZE,
};
use windows_sys::Win32::System::Kernel::{OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE};
use windows_sys::Win32::System::IO::{IO_STATUS_BLOCK, IO_STATUS_BLOCK_0};

pub(super) fn open_read(path: &Path) -> io::Result<File> {
    let file = open(path, FILE_GENERIC_READ, FILE_NON_DIRECTORY_FILE)?;
    // SAFETY: Query only; the handle stays alive throughout the call.
    if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK || !file.metadata()?.is_file()
    {
        return Err(invalid("read target is not a regular file"));
    }
    Ok(file)
}

fn open(path: &Path, access: u32, options: u32) -> io::Result<File> {
    let (root_path, mut name) = namespace_parts(path)?;
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let root = open_namespace_root(
        &root_path,
        if name.is_empty() {
            access
        } else {
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES
        },
        share,
    )?;
    if name.is_empty() {
        if options & FILE_NON_DIRECTORY_FILE != 0 {
            return Err(invalid("read target is a filesystem root directory"));
        }
        return Ok(root);
    }
    open_name(
        root.as_raw_handle(),
        &mut name,
        access,
        share,
        FILE_OPEN,
        options,
    )
}

pub(super) fn open_directory_snapshot(path: &Path) -> io::Result<File> {
    open(path, FILE_READ_ATTRIBUTES, FILE_DIRECTORY_FILE)
}

#[derive(PartialEq, Eq)]
pub(super) enum FileIdentity {
    Extended { volume: u64, id: [u8; 16] },
    Legacy { volume: u32, index: u64 },
}

pub(super) fn file_identity(file: &File) -> io::Result<FileIdentity> {
    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_FUNCTION, ERROR_INVALID_LEVEL, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO,
    };
    let mut info = std::mem::MaybeUninit::<FILE_ID_INFO>::uninit();
    // SAFETY: Correctly sized and aligned output buffer and a live file
    // handle; initialization is consumed only after the API reports success.
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            info.as_mut_ptr().cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        // Some network redirectors (including the local WSL test provider)
        // reject FileIdInfo while still implementing the documented legacy
        // file-index metadata used by std. Do not downgrade unrelated failures
        // or equate the legacy and extended identity representations.
        let error = io::Error::last_os_error();
        if !error.raw_os_error().is_some_and(|code| {
            [
                ERROR_INVALID_FUNCTION,
                ERROR_INVALID_LEVEL,
                ERROR_INVALID_PARAMETER,
                ERROR_NOT_SUPPORTED,
            ]
            .contains(&(code as u32))
        }) {
            return Err(error);
        }
        let mut legacy = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: The output buffer is correctly sized and initialized by the
        // API on success; the handle remains valid for the call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), legacy.as_mut_ptr()) } != 0 {
            // SAFETY: The successful API call initialized the structure.
            let legacy = unsafe { legacy.assume_init() };
            let volume = legacy.dwVolumeSerialNumber;
            let index = (u64::from(legacy.nFileIndexHigh) << 32) | u64::from(legacy.nFileIndexLow);
            if index == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "filesystem did not supply a usable legacy file identity",
                ));
            }
            return Ok(FileIdentity::Legacy { volume, index });
        }
        return Err(error);
    }
    // SAFETY: The successful API call initialized the complete structure.
    let info = unsafe { info.assume_init() };
    Ok(FileIdentity::Extended {
        volume: info.VolumeSerialNumber,
        id: info.FileId.Identifier,
    })
}

fn open_name(
    root: HANDLE,
    name: &mut [u16],
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
) -> io::Result<File> {
    let bytes =
        u16::try_from(size_of_val(name)).map_err(|_| invalid("native file name is too long"))?;
    let unicode = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: name.as_mut_ptr(),
    };
    let qos = SECURITY_QUALITY_OF_SERVICE {
        Length: size_of::<SECURITY_QUALITY_OF_SERVICE>() as u32,
        ImpersonationLevel: SecurityIdentification,
        ContextTrackingMode: SECURITY_STATIC_TRACKING,
        EffectiveOnly: 1,
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root,
        ObjectName: &unicode,
        Attributes: (OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE) as u32,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::addr_of!(qos).cast(),
    };
    let mut status_block = IO_STATUS_BLOCK {
        Anonymous: IO_STATUS_BLOCK_0 { Status: 0 },
        Information: 0,
    };
    let mut handle = ptr::null_mut();
    // SAFETY: All pointers reference live, correctly aligned windows-sys
    // structures/buffers for this synchronous call. No handle is inherited.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    if status < 0 {
        if status == STATUS_REPARSE_POINT_ENCOUNTERED {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "file path changed or contains a reparse point",
            ));
        }
        // SAFETY: Pure NTSTATUS to Win32 error conversion, no pointers.
        return Err(io::Error::from_raw_os_error(unsafe {
            RtlNtStatusToDosError(status) as i32
        }));
    }
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::other("native file open returned no handle"));
    }
    // SAFETY: A successful native open transfers its unique handle to File.
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn read_directory(path: &Path) -> io::Result<Vec<super::DirectoryEntry>> {
    let file = open(
        path,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_DIRECTORY_FILE,
    )?;
    read_directory_handle(&file)
}

fn read_directory_handle(directory: &File) -> io::Result<Vec<super::DirectoryEntry>> {
    // FILE_FULL_DIR_INFO requires 8-byte alignment, including every record.
    // A fixed 64-KiB page bounds the native buffer, not the number of entries.
    const PAGE_BYTES: usize = 64 * 1024;
    let mut page = vec![0u64; PAGE_BYTES / size_of::<u64>()];
    let mut class = FileFullDirectoryRestartInfo;
    let mut entries = Vec::new();
    loop {
        page.fill(0);
        // SAFETY: The handle stays alive; the writable buffer is correctly
        // aligned and its advertised capacity exactly matches its allocation.
        let success = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle(),
                class,
                page.as_mut_ptr().cast(),
                PAGE_BYTES as u32,
            )
        };
        if success == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(entries);
            }
            return Err(error);
        }
        parse_directory_page(&page, &mut entries)?;
        class = FileFullDirectoryInfo;
    }
}

fn parse_directory_page(page: &[u64], entries: &mut Vec<super::DirectoryEntry>) -> io::Result<()> {
    let length = size_of_val(page);
    let header = offset_of!(FILE_FULL_DIR_INFO, FileName);
    let mut offset = 0usize;
    loop {
        if offset.checked_add(header).is_none_or(|end| end > length) {
            return Err(invalid("directory record header exceeds native buffer"));
        }
        // SAFETY: The fields read here are within the checked fixed header;
        // do not read a whole variable-length struct or create a reference to it.
        let (next, attributes, name_bytes) = unsafe {
            let record = page
                .as_ptr()
                .cast::<u8>()
                .add(offset)
                .cast::<FILE_FULL_DIR_INFO>();
            (
                ptr::addr_of!((*record).NextEntryOffset).read_unaligned() as usize,
                ptr::addr_of!((*record).FileAttributes).read_unaligned(),
                ptr::addr_of!((*record).FileNameLength).read_unaligned() as usize,
            )
        };
        let record_length = header
            .checked_add(name_bytes)
            .ok_or_else(|| invalid("directory record length overflow"))?;
        if name_bytes == 0
            || name_bytes % size_of::<u16>() != 0
            || record_length > length - offset
            || (next != 0 && (next % 8 != 0 || next < record_length || next > length - offset))
        {
            return Err(invalid("invalid native directory record length"));
        }
        // SAFETY: The variable name is fully inside the checked allocation;
        // the record and filename offsets provide at least u16 alignment.
        let name = unsafe {
            std::slice::from_raw_parts(
                page.as_ptr()
                    .cast::<u8>()
                    .add(offset + header)
                    .cast::<u16>(),
                name_bytes / size_of::<u16>(),
            )
        };
        if name.contains(&0) {
            return Err(invalid("native directory name contains a NUL"));
        }
        let name = std::ffi::OsString::from_wide(name);
        if name != "." && name != ".." {
            entries.push(super::DirectoryEntry {
                name,
                is_dir: attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                is_link: attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
            });
        }
        if next == 0 {
            return Ok(());
        }
        offset += next;
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Separate the volume/share boundary from filesystem components without
/// canonicalizing or following any descendant. Device/relative paths and ADS
/// syntax are rejected before performing I/O, including for UNC paths.
fn namespace_parts(path: &Path) -> io::Result<(PathBuf, Vec<u16>)> {
    if !path.is_absolute() {
        return Err(invalid(
            "native file open requires an absolute filesystem path",
        ));
    }
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(invalid("native file open requires a filesystem prefix"));
    };
    match prefix.kind() {
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {}
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            relative_name(server)?;
            relative_name(share)?;
        }
        _ => return Err(invalid("native file open requires a disk or UNC prefix")),
    }
    if components.next() != Some(Component::RootDir) {
        return Err(invalid("native file open requires a rooted path"));
    }
    let mut root = PathBuf::from(prefix.as_os_str());
    root.push(r"\");
    let mut name = Vec::new();
    for component in components {
        match component {
            Component::Normal(part) => {
                if !name.is_empty() {
                    name.push(u16::from(b'\\'));
                }
                name.extend(relative_name(part)?);
            }
            _ => return Err(invalid("native file open requires normalized components")),
        }
    }
    Ok((root, name))
}

/// Only the filesystem root is opened through Win32, allowing its DOS/MUP
/// namespace resolution. All descendants use the same native no-reparse open.
/// This boundary never receives user-controlled filesystem components.
fn open_namespace_root(path: &Path, access: u32, share: u32) -> io::Result<File> {
    let (_, name) = namespace_parts(path)?;
    if !name.is_empty() {
        return Err(invalid("namespace root contains filesystem components"));
    }
    let root = std::fs::OpenOptions::new()
        .access_mode(access | SYNCHRONIZE)
        .share_mode(share)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(path)?;
    let metadata = root.metadata()?;
    // SAFETY: Query only; the owned root handle remains alive.
    if unsafe { GetFileType(root.as_raw_handle()) } != FILE_TYPE_DISK
        || !metadata.is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(invalid("filesystem root is not a regular directory"));
    }
    Ok(root)
}

mod write;
use write::relative_name;
pub(super) use write::{Stage, WriteDirectory};

#[cfg(test)]
mod directory_tests {
    use std::collections::BTreeSet;
    use std::fs;

    use super::*;

    #[test]
    fn native_directory_enumerates_unicode_across_pages_without_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let mut expected = BTreeSet::new();
        // The names alone exceed 64 KiB, before per-record headers. A single
        // successful page cannot accidentally satisfy this fixture.
        for index in 0..1200 {
            let name = format!("条目_{index:04}_{}🦀.txt", "目录".repeat(14));
            fs::write(directory.path().join(&name), "fixture").unwrap();
            expected.insert(std::ffi::OsString::from(name));
        }
        fs::create_dir(directory.path().join("子目录")).unwrap();
        expected.insert("子目录".into());
        let entries = read_directory(directory.path()).unwrap();
        assert_eq!(entries.len(), expected.len());
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.clone())
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert!(entries
            .iter()
            .any(|entry| entry.name == "子目录" && entry.is_dir));
        assert!(entries.iter().all(|entry| !entry.is_link));
    }

    #[test]
    fn native_directory_enumerates_open_handle_after_path_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        let parked = temp.path().join("parked");
        fs::create_dir(&original).unwrap();
        fs::write(original.join("original.txt"), "fixture").unwrap();
        let handle = open(
            &original,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_DIRECTORY_FILE,
        )
        .unwrap();
        fs::rename(&original, &parked).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("replacement.txt"), "fixture").unwrap();
        let entries = read_directory_handle(&handle).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "original.txt");
        let control = read_directory(&original).unwrap();
        assert_eq!(control.len(), 1);
        assert_eq!(control[0].name, "replacement.txt");
    }

    #[test]
    fn native_directory_empty_succeeds_but_file_and_missing_target_fail() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_directory(temp.path()).unwrap().is_empty());
        let path = temp.path().join("regular.txt");
        fs::write(&path, "fixture").unwrap();
        assert!(read_directory(&path).is_err());
        assert!(read_directory(&temp.path().join("missing")).is_err());
        assert!(read_directory_handle(&File::open(&path).unwrap()).is_err());
    }

    fn record_page(name: &[u16], next: u32) -> Vec<u64> {
        let header = offset_of!(FILE_FULL_DIR_INFO, FileName);
        let bytes = header + size_of_val(name);
        let mut page = vec![0u64; bytes.div_ceil(8)];
        // SAFETY: The allocated, aligned page covers the fixed header and name;
        // no reference to the variable-length FILE_FULL_DIR_INFO is formed.
        unsafe {
            let record = page.as_mut_ptr().cast::<FILE_FULL_DIR_INFO>();
            ptr::addr_of_mut!((*record).NextEntryOffset).write(next);
            ptr::addr_of_mut!((*record).FileAttributes).write(FILE_ATTRIBUTE_NORMAL);
            ptr::addr_of_mut!((*record).FileNameLength).write(size_of_val(name) as u32);
            ptr::copy_nonoverlapping(
                name.as_ptr(),
                page.as_mut_ptr().cast::<u8>().add(header).cast::<u16>(),
                name.len(),
            );
        }
        page
    }

    #[test]
    fn native_directory_parser_checks_lengths_offsets_and_nuls() {
        let name: Vec<u16> = "测试🦀.txt".encode_utf16().collect();
        let valid = record_page(&name, 0);
        let mut entries = Vec::new();
        parse_directory_page(&valid, &mut entries).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, std::ffi::OsString::from_wide(&name));
        for page in [
            Vec::new(),
            vec![0],
            record_page(&[], 0),
            record_page(&[0], 0),
            record_page(&name, 1),
            record_page(&name, 8),
            record_page(&name, u32::MAX - 7),
            record_page(&name, (valid.len() * 8) as u32),
        ] {
            assert!(parse_directory_page(&page, &mut Vec::new()).is_err());
        }
        for length in [1, u32::MAX - 1] {
            let mut page = valid.clone();
            // SAFETY: The valid fixture includes the entire fixed header.
            unsafe {
                let record = page.as_mut_ptr().cast::<FILE_FULL_DIR_INFO>();
                ptr::addr_of_mut!((*record).FileNameLength).write(length);
            }
            assert!(parse_directory_page(&page, &mut Vec::new()).is_err());
        }
        for dot in [".", ".."] {
            let mut entries = Vec::new();
            parse_directory_page(
                &record_page(&dot.encode_utf16().collect::<Vec<_>>(), 0),
                &mut entries,
            )
            .unwrap();
            assert!(entries.is_empty());
        }
    }
}
