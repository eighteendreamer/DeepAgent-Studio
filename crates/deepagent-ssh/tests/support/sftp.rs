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
    next_handle: usize,
}

impl MemoryFiles {
    pub fn new(counts: Arc<super::Counts>) -> Self {
        Self {
            files: counts.files.clone(),
            counts,
            handles: HashMap::new(),
            next_handle: 0,
        }
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

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _: FileAttributes,
    ) -> Result<Handle, Self::Error> {
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
        let mut files = self.files.write().unwrap();
        if files.contains_key(&newpath)
            || (newpath == "/reject-publish" && oldpath.ends_with(".part"))
        {
            return Err(StatusCode::Failure);
        }
        let bytes = files.remove(&oldpath).ok_or(StatusCode::NoSuchFile)?;
        files.insert(newpath, bytes);
        Ok(success(id))
    }

    async fn remove(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        self.files
            .write()
            .unwrap()
            .remove(&path)
            .ok_or(StatusCode::NoSuchFile)?;
        Ok(success(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle).ok_or(StatusCode::NoSuchFile)?;
        Ok(success(id))
    }
}
