//! Deliberately small, in-memory SFTP fixture; never resolves host filesystem paths.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
};

pub struct MemoryFiles {
    counts: Arc<super::Counts>,
    files: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    handles: HashMap<String, String>,
    directories: HashMap<String, std::collections::VecDeque<File>>,
    next_handle: usize,
}

impl MemoryFiles {
    pub fn new(counts: Arc<super::Counts>) -> Self {
        Self {
            files: counts.files.clone(),
            counts,
            handles: HashMap::new(),
            directories: HashMap::new(),
            next_handle: 0,
        }
    }

    fn rejection(&self, id: u32, operation: &str) -> Option<Status> {
        self.counts
            .status_messages
            .read()
            .unwrap()
            .get(operation)
            .map(|message| Status {
                id,
                status_code: StatusCode::PermissionDenied,
                error_message: message.clone(),
                language_tag: String::new(),
            })
    }
}

fn success(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: String::new(),
    }
}

impl russh_sftp::server::Handler for MemoryFiles {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        if let Some(attrs) = self
            .counts
            .file_attributes
            .read()
            .unwrap()
            .get(&path)
            .cloned()
        {
            return Ok(Attrs { id, attrs });
        }
        let attrs = match path.as_str() {
            "/denied" => return Err(StatusCode::PermissionDenied),
            "/unknown" => FileAttributes::empty(),
            "/huge" => FileAttributes {
                size: Some(u64::MAX),
                permissions: Some(0o100644),
                ..Default::default()
            },
            "/link" => FileAttributes {
                size: Some(22),
                permissions: Some(0o120777),
                uid: Some(1001),
                gid: Some(1002),
                ..Default::default()
            },
            _ if self.counts.directories.read().unwrap().contains(&path) || path == "/" => {
                FileAttributes {
                    permissions: Some(0o040750),
                    ..Default::default()
                }
            }
            _ => {
                let files = self.files.read().unwrap();
                let bytes = files.get(&path).ok_or(StatusCode::NoSuchFile)?;
                FileAttributes {
                    size: Some(bytes.len() as u64 + if path == "/short" { 10 } else { 0 }),
                    permissions: Some(0o100640),
                    uid: Some(1001),
                    gid: Some(1002),
                    mtime: Some(1_700_000_000),
                    ..Default::default()
                }
            }
        };
        Ok(Attrs { id, attrs })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.lstat(
            id,
            if path == "/link" {
                "/home/fixture/file.txt".into()
            } else {
                path
            },
        )
        .await
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        if let Some(canonical) = self.counts.canonical_paths.read().unwrap().get(&path) {
            return Ok(Name {
                id,
                files: vec![File::dummy(canonical)],
            });
        }
        if path == "/" || self.counts.directories.read().unwrap().contains(&path) {
            return Ok(Name {
                id,
                files: vec![File::dummy(path)],
            });
        }
        if path != "./file.txt" {
            return Err(StatusCode::NoSuchFile);
        }
        Ok(Name {
            id,
            files: vec![File {
                filename: "/home/fixture/file.txt".into(),
                longname: String::new(),
                attrs: FileAttributes::default(),
            }],
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let mode = attrs.permissions.ok_or(StatusCode::Failure)?;
        assert!(attrs.size.is_none() && attrs.uid.is_none() && attrs.gid.is_none());
        self.counts
            .chmod_requests
            .write()
            .unwrap()
            .push((path.clone(), mode));
        if self
            .counts
            .reject_chmod_paths
            .read()
            .unwrap()
            .contains(&path)
        {
            return Err(StatusCode::PermissionDenied);
        }
        let mut current = self.lstat(id, path.clone()).await?.attrs;
        current.permissions = Some((current.permissions.unwrap_or(0) & !0o7777) | (mode & 0o7777));
        self.counts
            .file_attributes
            .write()
            .unwrap()
            .insert(path, current);
        Ok(success(id))
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        self.counts
            .file_opens
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if filename == "/denied" {
            return Err(StatusCode::PermissionDenied);
        }
        {
            let mut files = self.files.write().unwrap();
            if !flags.contains(OpenFlags::CREATE) && !files.contains_key(&filename) {
                return Err(StatusCode::NoSuchFile);
            }
            if flags.contains(OpenFlags::EXCLUDE) && files.contains_key(&filename) {
                return Err(StatusCode::Failure);
            }
            let bytes = files.entry(filename.clone()).or_default();
            if flags.contains(OpenFlags::TRUNCATE) {
                bytes.clear();
            }
        }
        if filename.ends_with(".part") {
            let gate = self.counts.stage_open_gate.read().unwrap().clone();
            self.counts.stage_created.notify_one();
            if let Some(gate) = gate {
                gate.notified().await;
            }
        }
        self.next_handle += 1;
        let handle = self.next_handle.to_string();
        self.handles.insert(handle.clone(), filename);
        Ok(Handle { id, handle })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        if let Some(status) = self.rejection(id, "write") {
            return Ok(status);
        }
        let path = self.handles.get(&handle).ok_or(StatusCode::NoSuchFile)?;
        let offset = usize::try_from(offset).map_err(|_| StatusCode::Failure)?;
        let end = offset.checked_add(data.len()).ok_or(StatusCode::Failure)?;
        // Bound even fixture allocations: reject malformed/unexpected offsets.
        if end > 4 * 1024 * 1024 {
            return Err(StatusCode::Failure);
        }
        let mut files = self.files.write().unwrap();
        let bytes = files.get_mut(path).ok_or(StatusCode::NoSuchFile)?;
        bytes.resize(bytes.len().max(end), 0);
        bytes[offset..end].copy_from_slice(&data);
        Ok(success(id))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let path = self
            .handles
            .get(&handle)
            .ok_or(StatusCode::NoSuchFile)?
            .clone();
        self.lstat(id, path).await
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let path = self.handles.get(&handle).ok_or(StatusCode::NoSuchFile)?;
        let files = self.files.read().unwrap();
        let bytes = files.get(path).ok_or(StatusCode::NoSuchFile)?;
        let offset = usize::try_from(offset).map_err(|_| StatusCode::Failure)?;
        if offset >= bytes.len() {
            return Err(StatusCode::Eof);
        }
        Ok(Data {
            id,
            data: bytes[offset..bytes.len().min(offset + len as usize)].to_vec(),
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let operation = if self.counts.directories.read().unwrap().contains(&oldpath) {
            "rename_directory"
        } else {
            "rename"
        };
        if let Some(status) = self.rejection(id, operation) {
            return Ok(status);
        }
        let directory = {
            let files = self.files.read().unwrap();
            let directories = self.counts.directories.read().unwrap();
            if files.contains_key(&newpath)
                || directories.contains(&newpath)
                || (newpath == "/reject-publish" && oldpath.ends_with(".part"))
                || self
                    .counts
                    .reject_publish_paths
                    .read()
                    .unwrap()
                    .contains(&newpath)
            {
                return Err(StatusCode::Failure);
            }
            directories.contains(&oldpath)
        };
        if directory {
            self.counts
                .directory_renames
                .write()
                .unwrap()
                .push((oldpath.clone(), newpath.clone()));
            let gate = self.counts.directory_rename_gate.write().unwrap().take();
            self.counts.directory_rename_started.notify_one();
            if let Some(gate) = gate {
                gate.notified().await;
            }
            if (oldpath.ends_with(".directory-stage")
                && self
                    .counts
                    .reject_directory_publish
                    .load(std::sync::atomic::Ordering::SeqCst))
                || (oldpath.ends_with(".directory-backup")
                    && self
                        .counts
                        .reject_directory_restore
                        .load(std::sync::atomic::Ordering::SeqCst))
            {
                return Err(StatusCode::Failure);
            }
            let mut files = self.files.write().unwrap();
            let mut directories = self.counts.directories.write().unwrap();
            // Recheck after the delivery gate, just as a server rejects a raced target.
            if files.contains_key(&newpath) || directories.contains(&newpath) {
                return Err(StatusCode::Failure);
            }
            let prefix = format!("{oldpath}/");
            let moves: Vec<_> = directories
                .iter()
                .filter(|path| *path == &oldpath || path.starts_with(&prefix))
                .cloned()
                .collect();
            for path in moves {
                directories.remove(&path);
                directories.insert(format!("{newpath}{}", &path[oldpath.len()..]));
            }
            let moves: Vec<_> = files
                .keys()
                .filter(|path| path.starts_with(&prefix))
                .cloned()
                .collect();
            for path in moves {
                let value = files.remove(&path).unwrap();
                files.insert(format!("{newpath}{}", &path[oldpath.len()..]), value);
            }
            let mut attributes = self.counts.file_attributes.write().unwrap();
            let moves: Vec<_> = attributes
                .keys()
                .filter(|path| *path == &oldpath || path.starts_with(&prefix))
                .cloned()
                .collect();
            for path in moves {
                let value = attributes.remove(&path).unwrap();
                attributes.insert(format!("{newpath}{}", &path[oldpath.len()..]), value);
            }
            return Ok(success(id));
        }
        let mut files = self.files.write().unwrap();
        let bytes = files.remove(&oldpath).ok_or(StatusCode::NoSuchFile)?;
        files.insert(newpath, bytes);
        Ok(success(id))
    }

    async fn remove(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        if path.ends_with(".backup")
            && self
                .counts
                .reject_file_backup_cleanup
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(StatusCode::PermissionDenied);
        }
        if path.contains(".directory-backup/")
            && self
                .counts
                .reject_directory_cleanup
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(StatusCode::PermissionDenied);
        }
        self.files
            .write()
            .unwrap()
            .remove(&path)
            .ok_or(StatusCode::NoSuchFile)?;
        self.counts.file_attributes.write().unwrap().remove(&path);
        Ok(success(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let parent = path.rsplit_once('/').map(|p| p.0).unwrap_or("");
        {
            let files = self.files.read().unwrap();
            let mut directories = self.counts.directories.write().unwrap();
            if (!parent.is_empty() && !directories.contains(parent))
                || files.contains_key(&path)
                || !directories.insert(path.clone())
            {
                return Err(StatusCode::Failure);
            }
        }
        if path.ends_with(".directory-stage") {
            let gate = self.counts.directory_create_gate.write().unwrap().take();
            self.counts.directory_create_started.notify_one();
            if let Some(gate) = gate {
                gate.notified().await;
            }
        }
        Ok(success(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        if path.ends_with(".directory-backup")
            && self
                .counts
                .reject_directory_cleanup
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(StatusCode::PermissionDenied);
        }
        let prefix = format!("{path}/");
        let files = self.files.read().unwrap();
        let mut directories = self.counts.directories.write().unwrap();
        if files.keys().any(|entry| entry.starts_with(&prefix))
            || directories.iter().any(|entry| entry.starts_with(&prefix))
        {
            return Err(StatusCode::Failure);
        }
        if !directories.remove(&path) {
            return Err(StatusCode::NoSuchFile);
        }
        self.counts.file_attributes.write().unwrap().remove(&path);
        Ok(success(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        if self.directories.remove(&handle).is_some() {
            self.counts
                .directory_closes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(status) = self.rejection(id, "close_directory") {
                return Ok(status);
            }
            if self
                .counts
                .reject_directory_close
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(StatusCode::Failure);
            }
            return Ok(success(id));
        }
        self.handles.remove(&handle).ok_or(StatusCode::NoSuchFile)?;
        Ok(success(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        if !self.lstat(id, path.clone()).await?.attrs.is_dir() {
            return Err(StatusCode::NoSuchFile);
        }
        let override_entries = self
            .counts
            .directory_entries
            .read()
            .unwrap()
            .get(&path)
            .cloned();
        let populate_attributes = override_entries.is_none();
        let mut entries = if let Some(entries) = override_entries {
            entries
        } else {
            let mut entries = std::collections::BTreeMap::new();
            let prefix = format!("{}/", path.trim_end_matches('/'));
            for file in self.files.read().unwrap().keys() {
                if let Some(name) = file
                    .strip_prefix(&prefix)
                    .filter(|name| !name.contains('/'))
                {
                    entries.insert(name.to_string(), File::dummy(name));
                }
            }
            for dir in self.counts.directories.read().unwrap().iter() {
                if let Some(name) = dir
                    .strip_prefix(&prefix)
                    .filter(|name| !name.is_empty() && !name.contains('/'))
                {
                    entries.insert(name.to_string(), File::dummy(name));
                }
            }
            entries.into_values().collect()
        };
        if populate_attributes {
            for entry in &mut entries {
                let full = format!("{}/{}", path.trim_end_matches('/'), entry.filename);
                entry.attrs = self.lstat(id, full).await?.attrs;
            }
        }
        self.next_handle += 1;
        let handle = format!("dir-{}", self.next_handle);
        self.directories.insert(handle.clone(), entries.into());
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        self.counts
            .directory_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let gate = self.counts.directory_read_gate.read().unwrap().clone();
        self.counts.directory_read_started.notify_one();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        let entries = self
            .directories
            .get_mut(&handle)
            .ok_or(StatusCode::NoSuchFile)?;
        if entries.is_empty() {
            return Err(StatusCode::Eof);
        }
        if self
            .counts
            .repeat_directory_page
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(Name {
                id,
                files: entries.iter().take(2).cloned().collect(),
            });
        }
        let files = entries.drain(..entries.len().min(2)).collect();
        Ok(Name { id, files })
    }
}
