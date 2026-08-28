//! Inventory managed file roots into SQLite without storing large blobs there.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::managed_file_store::{ManagedFileRecord, ManagedFileStore};
use deepagent_persistence::Database;

pub struct ManagedFileInventory {
    db: Arc<Database>,
}

impl ManagedFileInventory {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub fn refresh(&self, category: &str, root: &Path) -> Result<usize> {
        let mut records = Vec::new();
        if root.exists() {
            scan(root, root, category, &mut records)?;
        }
        ManagedFileStore::new(&self.db).replace_category(category, root, &records)?;
        Ok(records.len())
    }

    pub fn refresh_all(&self, roots: &[(String, PathBuf)]) -> Result<usize> {
        roots.iter().try_fold(0usize, |total, (category, root)| {
            self.refresh(category, root).map(|count| total + count)
        })
    }
}

fn scan(
    root: &Path,
    directory: &Path,
    category: &str,
    output: &mut Vec<ManagedFileRecord>,
) -> Result<()> {
    let mut entries = std::fs::read_dir(directory)
        .map_err(|error| CoreError::other(error.to_string()))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| CoreError::other(error.to_string()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| CoreError::other(error.to_string()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            scan(root, &path, category, output)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| CoreError::other(error.to_string()))?;
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        output.push(ManagedFileRecord {
            category: category.to_string(),
            relative_path: relative,
            root_path: root.to_string_lossy().to_string(),
            byte_size: metadata.len(),
            modified_at: metadata.modified().ok().and_then(system_time_ms),
            digest: None,
            status: "present".into(),
            updated_at: now_ms(),
        });
    }
    Ok(())
}

fn system_time_ms(value: std::time::SystemTime) -> Option<i64> {
    value
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn now_ms() -> i64 {
    system_time_ms(std::time::SystemTime::now()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_records_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("nested")).unwrap();
        std::fs::write(root.path().join("nested/a.txt"), b"abc").unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let inventory = ManagedFileInventory::new(db.clone());
        assert_eq!(inventory.refresh("attachments", root.path()).unwrap(), 1);
        let records = ManagedFileStore::new(&db)
            .list(Some("attachments"))
            .unwrap();
        assert_eq!(records[0].relative_path, "nested/a.txt");
        assert_eq!(records[0].byte_size, 3);
    }
}
