//! Deliberately small, in-memory SFTP fixture; never resolves host filesystem paths.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use russh_sftp::protocol::{FileAttributes, Handle, OpenFlags, Status, StatusCode};

pub struct MemoryFiles {
    files: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    handles: HashMap<String, String>,
    next_handle: usize,
}

impl MemoryFiles {
    pub fn new(files: Arc<RwLock<HashMap<String, Vec<u8>>>>) -> Self {
        Self {
            files,
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
        if !flags.contains(OpenFlags::WRITE) {
            return Err(StatusCode::OpUnsupported);
        }
        let mut files = self.files.write().unwrap();
        if flags.contains(OpenFlags::EXCLUDE) && files.contains_key(&filename) {
            return Err(StatusCode::Failure);
        }
        let bytes = files.entry(filename.clone()).or_default();
        if flags.contains(OpenFlags::TRUNCATE) {
            bytes.clear();
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

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle).ok_or(StatusCode::NoSuchFile)?;
        Ok(success(id))
    }
}
