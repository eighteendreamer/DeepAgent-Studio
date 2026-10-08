//! One rule for which SQLite file a host process opens.
//!
//! The desktop shell keeps its data next to the installed executable, while the
//! historical CLI default was `<workspace>/.deepagent`. A terminal host that is
//! meant to show the same conversations as the desktop must resolve to the same
//! file, so the precedence lives here once instead of in each binary.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub const DATABASE_FILE: &str = "deepagent.db";
/// Workspace-local data directory kept for single-project use.
pub const WORKSPACE_DIR: &str = ".deepagent";

/// Every input the rule needs, injected so the precedence is testable without
/// touching the process environment or the file system.
#[derive(Debug, Clone)]
pub struct DatabasePathInput<'a> {
    /// `DEEPAGENT_DB_PATH`: an explicit database file.
    pub database_path_env: Option<&'a OsStr>,
    /// `DEEPAGENT_DATA_ROOT`: an explicit data directory.
    pub data_root_env: Option<&'a OsStr>,
    /// The installed shell's database, present only when that file exists.
    pub installed_database: Option<&'a Path>,
    /// Directory the host was started in.
    pub workspace: &'a Path,
}

pub fn choose_database_path(input: &DatabasePathInput<'_>) -> PathBuf {
    let explicit =
        |value: Option<&OsStr>| value.filter(|value| !value.is_empty()).map(PathBuf::from);
    if let Some(path) = explicit(input.database_path_env) {
        return path;
    }
    if let Some(root) = explicit(input.data_root_env) {
        return root.join(DATABASE_FILE);
    }
    if let Some(installed) = input.installed_database {
        return installed.to_path_buf();
    }
    input.workspace.join(WORKSPACE_DIR).join(DATABASE_FILE)
}

/// Resolve the database file for a host started in `workspace`.
pub fn resolve_database_path(workspace: &Path) -> PathBuf {
    let database_path_env: Option<OsString> = std::env::var_os("DEEPAGENT_DB_PATH");
    let data_root_env: Option<OsString> = std::env::var_os("DEEPAGENT_DATA_ROOT");
    let installed = installed_database();
    choose_database_path(&DatabasePathInput {
        database_path_env: database_path_env.as_deref(),
        data_root_env: data_root_env.as_deref(),
        installed_database: installed.as_deref(),
        workspace,
    })
}

/// The desktop shell's database, when this binary is installed next to it.
fn installed_database() -> Option<PathBuf> {
    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))?;
    let candidate = install_dir.join("data").join(DATABASE_FILE);
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::{choose_database_path, DatabasePathInput, DATABASE_FILE, WORKSPACE_DIR};
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    const INSTALLED: &str = "/install/data/deepagent.db";

    fn choose(
        database_env: Option<&OsStr>,
        data_root_env: Option<&OsStr>,
        installed: Option<&Path>,
        workspace: &Path,
    ) -> PathBuf {
        choose_database_path(&DatabasePathInput {
            database_path_env: database_env,
            data_root_env,
            installed_database: installed,
            workspace,
        })
    }

    #[test]
    fn explicit_database_file_wins_over_everything() {
        let workspace = PathBuf::from("/work/repo");
        let chosen = choose(
            Some(OsStr::new("/data/other.db")),
            Some(OsStr::new("/ignored")),
            Some(Path::new(INSTALLED)),
            &workspace,
        );
        assert_eq!(chosen, PathBuf::from("/data/other.db"));
    }

    #[test]
    fn explicit_data_root_beats_the_installed_shell() {
        let workspace = PathBuf::from("/work/repo");
        let chosen = choose(
            None,
            Some(OsStr::new("/mnt/shared")),
            Some(Path::new(INSTALLED)),
            &workspace,
        );
        assert_eq!(chosen, PathBuf::from("/mnt/shared").join(DATABASE_FILE));
    }

    #[test]
    fn installed_shell_data_beats_the_workspace_default() {
        let workspace = PathBuf::from("/work/repo");
        let chosen = choose(None, None, Some(Path::new(INSTALLED)), &workspace);
        assert_eq!(chosen, PathBuf::from(INSTALLED));
    }

    #[test]
    fn falls_back_to_the_historical_workspace_directory() {
        let workspace = PathBuf::from("/work/repo");
        let chosen = choose(None, None, None, &workspace);
        assert_eq!(chosen, workspace.join(WORKSPACE_DIR).join(DATABASE_FILE));
    }

    #[test]
    fn empty_environment_values_are_ignored_not_treated_as_paths() {
        let workspace = PathBuf::from("/work/repo");
        let chosen = choose(Some(OsStr::new("")), Some(OsStr::new("")), None, &workspace);
        assert_eq!(chosen, workspace.join(WORKSPACE_DIR).join(DATABASE_FILE));
    }
}
