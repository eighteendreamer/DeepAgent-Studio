//! Session-stable root identity; operation pins reuse the existing directory I/O.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::physical_with_missing_suffix;

#[derive(Debug)]
pub(super) struct RootAnchor {
    pub physical: PathBuf,
    ancestor: PathBuf,
    original: File,
    created_root: OnceLock<File>,
}

pub(super) struct RootPin {
    #[cfg(windows)]
    directory: super::windows::WriteDirectory,
    #[cfg(not(windows))]
    directory: File,
}

impl RootPin {
    fn open(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        let directory = super::windows::WriteDirectory::pin(path)?;
        #[cfg(not(windows))]
        let directory = snapshot(path)?;
        Ok(Self { directory })
    }

    fn file(&self) -> &File {
        #[cfg(windows)]
        {
            self.directory.file()
        }
        #[cfg(not(windows))]
        {
            &self.directory
        }
    }

    fn validate(&self) -> io::Result<()> {
        #[cfg(windows)]
        self.directory.validate()?;
        Ok(())
    }
}

impl RootAnchor {
    pub fn capture(logical: &Path) -> io::Result<Self> {
        let physical = physical_with_missing_suffix(&std::path::absolute(logical)?)?;
        let mut ancestor = physical.clone();
        let original = loop {
            match snapshot(&ancestor) {
                Ok(file) => break file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if !ancestor.pop() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        };
        // Do not silently accept an unidentifiable filesystem as a trusted root.
        same_file(&original, &original)?;
        if physical_with_missing_suffix(logical)? != physical {
            return Err(changed());
        }
        Ok(Self {
            physical,
            ancestor,
            original,
            created_root: OnceLock::new(),
        })
    }

    pub fn pin(&self, logical: &Path) -> io::Result<RootPin> {
        let ancestor_pin = RootPin::open(&self.ancestor)?;
        if !same_file(&self.original, ancestor_pin.file())? {
            return Err(changed());
        }
        let pin = if self.ancestor == self.physical {
            ancestor_pin
        } else {
            match RootPin::open(&self.physical) {
                Ok(pin) => {
                    // Initial missing roots remain bound to their original
                    // ancestor; once created, bind their own identity once.
                    if self.created_root.get().is_none() {
                        let _ = self.created_root.set(snapshot(&self.physical)?);
                    }
                    if !same_file(
                        self.created_root.get().expect("root identity set"),
                        pin.file(),
                    )? {
                        return Err(changed());
                    }
                    pin
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if self.created_root.get().is_some() {
                        return Err(changed());
                    }
                    ancestor_pin
                }
                Err(error) => return Err(error),
            }
        };
        pin.validate()?;
        if physical_with_missing_suffix(logical)? != self.physical {
            return Err(changed());
        }
        Ok(pin)
    }
}

fn snapshot(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    let file = super::windows::open_directory_snapshot(path)?;
    #[cfg(not(windows))]
    let file = File::open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(io::Error::other(
            "workspace root ancestor is not a directory",
        ));
    }
    Ok(file)
}

fn same_file(left: &File, right: &File) -> io::Result<bool> {
    #[cfg(windows)]
    {
        Ok(super::windows::file_identity(left)? == super::windows::file_identity(right)?)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = left.metadata()?;
        let right = right.metadata()?;
        Ok((left.dev(), left.ino()) == (right.dev(), right.ino()))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (left, right);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem identity unavailable",
        ))
    }
}

fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "workspace root identity changed",
    )
}
