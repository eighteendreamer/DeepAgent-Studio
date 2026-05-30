//! Active-project identity for the UI (Phase C — desktop project header).
//!
//! The product model: a **project is a folder**. The UI shows the project
//! folder *name*; the agent's file operations are rooted at this folder (the
//! confinement itself is enforced by the built-in file tools' `WorkspaceRoot`
//! guard during a chat run — see `deepagent-builtins`). This service simply
//! reports *which* folder is the active project so the frontend can display its
//! name and path.

use std::path::PathBuf;

use crate::dto::WorkspaceInfoDto;

/// Reports the active project folder (name + absolute path).
pub struct WorkspaceService {
    root: PathBuf,
}

impl WorkspaceService {
    /// Build over the project root directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The active project folder name (last path component). Falls back to the
    /// full path string when there is no final component (e.g. a drive root).
    pub fn project_name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    /// The absolute project root path (display form).
    pub fn root_display(&self) -> String {
        self.root.display().to_string()
    }

    /// The project info DTO the UI binds to (folder name + path).
    pub fn info(&self) -> WorkspaceInfoDto {
        WorkspaceInfoDto {
            name: self.project_name(),
            path: self.root_display(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_folder_name() {
        let svc = WorkspaceService::new("/work/小红草");
        assert_eq!(svc.project_name(), "小红草");
    }

    #[test]
    fn info_carries_name_and_path() {
        let svc = WorkspaceService::new("/home/me/Looker-v2");
        let info = svc.info();
        assert_eq!(info.name, "Looker-v2");
        assert_eq!(
            info.path,
            PathBuf::from("/home/me/Looker-v2").display().to_string()
        );
    }

    #[test]
    fn root_display_is_the_root() {
        let svc = WorkspaceService::new("/proj/demo");
        assert_eq!(
            svc.root_display(),
            PathBuf::from("/proj/demo").display().to_string()
        );
    }
}
