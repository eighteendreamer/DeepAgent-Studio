//! Platform I/O for the single observed-version mutation/commit boundary.

use std::fs::File;
use std::io;
use std::path::PathBuf;

pub(crate) struct WriteTarget {
    path: PathBuf,
    observation: Option<(super::WorkspaceRoot, PathBuf)>,
    #[cfg(windows)]
    parent: super::windows::WriteDirectory,
}

pub(crate) struct StagedWrite {
    #[cfg(windows)]
    inner: super::windows::Stage,
    #[cfg(not(windows))]
    inner: tempfile::NamedTempFile,
}

impl WriteTarget {
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub(super) fn open(path: PathBuf, create_parents: bool) -> io::Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("write target has no parent"))?;
        if path.file_name().is_none() {
            return Err(io::Error::other("write target has no file name"));
        }
        #[cfg(windows)]
        let parent = super::windows::WriteDirectory::open(parent, create_parents)?;
        #[cfg(not(windows))]
        if create_parents {
            // Portable string-path behavior is retained, not claimed to have
            // the Windows handle-relative race protection.
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path,
            observation: None,
            #[cfg(windows)]
            parent,
        })
    }

    pub(super) fn observe_through(mut self, root: super::WorkspaceRoot, path: PathBuf) -> Self {
        self.observation = Some((root, path));
        self
    }

    pub(crate) fn read_current(&self) -> io::Result<Option<String>> {
        use std::io::Read;
        let (root, path) = self.observation.as_ref().ok_or_else(|| {
            io::Error::other("entry target has no authorized mutation observation")
        })?;
        match root.open_write_observation(path)? {
            Some(mut file) => {
                let mut content = String::new();
                file.read_to_string(&mut content)?;
                Ok(Some(content))
            }
            None => Ok(None),
        }
    }

    pub(crate) fn delete_entry(&self, recursive: bool) -> io::Result<bool> {
        #[cfg(windows)]
        {
            self.parent.delete_entry(&self.path, recursive)
        }
        #[cfg(not(windows))]
        {
            let metadata = std::fs::symlink_metadata(&self.path)?;
            let is_directory = metadata.is_dir();
            if is_directory {
                if !recursive {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "target is a directory; pass recursive=true to delete it",
                    ));
                }
                std::fs::remove_dir_all(&self.path)?;
            } else {
                std::fs::remove_file(&self.path)?;
            }
            Ok(is_directory)
        }
    }

    pub(crate) fn stage(&self) -> io::Result<StagedWrite> {
        let parent = self.path.parent().expect("validated parent");
        #[cfg(windows)]
        let inner = self
            .parent
            .stage(parent, self.path.file_name().expect("validated file name"))?;
        #[cfg(not(windows))]
        let inner = tempfile::Builder::new()
            .prefix(".deepagent-write-")
            .tempfile_in(parent)?;
        Ok(StagedWrite { inner })
    }
}

impl StagedWrite {
    pub(crate) fn file_mut(&mut self) -> &mut File {
        #[cfg(windows)]
        {
            &mut self.inner.file
        }
        #[cfg(not(windows))]
        {
            self.inner.as_file_mut()
        }
    }

    pub(crate) fn publish(self, target: &WriteTarget, replace: bool) -> io::Result<()> {
        #[cfg(windows)]
        {
            self.inner.publish(
                &target.parent,
                target.path.file_name().expect("validated file name"),
                replace,
            )
        }
        #[cfg(not(windows))]
        {
            let path = self.inner.into_temp_path();
            if replace {
                std::fs::rename(&path, &target.path)
            } else {
                deepagent_persistence::fs_commit::rename_no_replace(&path, &target.path)
            }
        }
    }
}
