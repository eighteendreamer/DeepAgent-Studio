//! One rule for where a host keeps its data.
//!
//! The desktop shell historically wrote everything next to the installed
//! executable and the CLI defaulted to `<workspace>/.deepagent`, so a terminal
//! host could not see the conversations, skills or credentials the desktop
//! manages. Both hosts now resolve the same user-level root: an explicit
//! `DEEPAGENT_DATA_ROOT` (or `DEEPAGENT_DB_PATH` for just the database),
//! otherwise `~/.deepagent` — the same shape codex uses for `CODEX_HOME`
//! (`借鉴/codex/codex-rs/utils/home-dir/src/lib.rs`).
//!
//! `UserPaths` is the only place that spells out the layout, so a new host gets
//! every directory without re-deriving path fragments by hand.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};

pub const DATABASE_FILE: &str = "deepagent.db";
pub const RUNTIME_LOG_FILE: &str = "runtime-logs.db";
/// Directory name under the user's home directory.
pub const HOME_DIR_NAME: &str = ".deepagent";
pub const DATA_ROOT_ENV: &str = "DEEPAGENT_DATA_ROOT";
pub const DB_PATH_ENV: &str = "DEEPAGENT_DB_PATH";

/// The user's home directory, from `HOME` then `USERPROFILE`.
pub fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn non_empty(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// Every input the root rule needs, injected so the precedence is testable
/// without touching the process environment.
#[derive(Debug, Clone)]
pub struct DataRootInput<'a> {
    /// `DEEPAGENT_DATA_ROOT`: an explicit data directory.
    pub data_root_env: Option<&'a OsStr>,
    /// The user's home directory, when the environment names one.
    pub home: Option<&'a Path>,
}

pub fn choose_data_root(input: &DataRootInput<'_>) -> Result<PathBuf> {
    if let Some(root) = non_empty(input.data_root_env.map(OsString::from)) {
        return Ok(root);
    }
    input
        .home
        .map(|home| home.join(HOME_DIR_NAME).to_path_buf())
        .ok_or_else(|| {
            CoreError::invalid(format!(
                "cannot resolve the DeepAgent data root: neither HOME nor USERPROFILE is set, \
                 and {DATA_ROOT_ENV} is unset"
            ))
        })
}

/// Resolve the directory every host writes to.
pub fn resolve_data_root() -> Result<PathBuf> {
    let data_root_env: Option<OsString> = std::env::var_os(DATA_ROOT_ENV);
    let home = home_dir();
    choose_data_root(&DataRootInput {
        data_root_env: data_root_env.as_deref(),
        home: home.as_deref(),
    })
}

/// Every writable location under one data root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPaths {
    pub root: PathBuf,
    pub database: PathBuf,
    pub runtime_logs: PathBuf,
    pub files: PathBuf,
    pub attachments: PathBuf,
    pub recordings: PathBuf,
    /// Where personal plugin directories land (`PluginRoots::personal`).
    pub plugins: PathBuf,
    /// Anchor for plugin state and plugin runtime data (`PluginService::new`).
    pub plugin_state_root: PathBuf,
    pub plugin_data: PathBuf,
    pub skills: PathBuf,
    pub skills_user: PathBuf,
    pub skills_marketplace: PathBuf,
    pub runtimes: PathBuf,
    pub mcp: PathBuf,
    pub cache: PathBuf,
    pub vision_cache: PathBuf,
    pub tool_results: PathBuf,
    pub artifacts: PathBuf,
    pub ssh: PathBuf,
    /// Retired half-installed assets awaiting review, never deleted on sight.
    pub retired: PathBuf,
}

impl UserPaths {
    /// Build the layout over an explicit root.
    pub fn under(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let files = root.join("files");
        let skills_user = files.join("skills").join("user");
        let cache = files.join("cache");
        Self {
            database: root.join(DATABASE_FILE),
            runtime_logs: root.join(RUNTIME_LOG_FILE),
            plugins: files.join("plugins"),
            plugin_state_root: root.join("plugins"),
            plugin_data: root.join("plugins").join("data"),
            skills: files.join("skills"),
            skills_marketplace: skills_user.join("marketplace"),
            runtimes: files.join("runtimes"),
            mcp: files.join("mcp"),
            tool_results: files.join("tool-results"),
            artifacts: files.join("artifacts"),
            attachments: files.join("attachments"),
            recordings: files.join("recordings"),
            ssh: root.join("deepagent-ssh"),
            retired: root.join(".retired"),
            vision_cache: cache.join("vision"),
            skills_user,
            cache,
            files,
            root,
        }
    }

    /// Build the layout for the resolved data root.
    pub fn resolve() -> Result<Self> {
        Ok(Self::under(resolve_data_root()?))
    }

    /// Create the managed directories so a first run on a clean machine never
    /// sees a missing root.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in self.managed_dirs() {
            std::fs::create_dir_all(&dir).map_err(|error| {
                CoreError::Persistence(format!(
                    "create managed directory {}: {error}",
                    dir.display()
                ))
            })?;
        }
        Ok(())
    }

    /// Directories the host owns and may create; the data root itself is not
    /// included because the database file is its first writer.
    pub fn managed_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.files.clone(),
            self.attachments.clone(),
            self.recordings.clone(),
            self.plugins.clone(),
            self.skills.clone(),
            self.skills_user.clone(),
            self.skills_marketplace.clone(),
            self.runtimes.clone(),
            self.mcp.clone(),
            self.cache.clone(),
            self.vision_cache.clone(),
            self.tool_results.clone(),
            self.artifacts.clone(),
        ]
    }

    /// The install ledger lives beside the root's state files, not under `files`.
    pub fn install_ledger(&self) -> PathBuf {
        self.root.join("install-ledger.json")
    }
}

/// Every input the database rule needs, injected for testing.
#[derive(Debug, Clone)]
pub struct DatabasePathInput<'a> {
    /// `DEEPAGENT_DB_PATH`: an explicit database file.
    pub database_path_env: Option<&'a OsStr>,
    /// `DEEPAGENT_DATA_ROOT`: an explicit data directory.
    pub data_root_env: Option<&'a OsStr>,
    /// The user's home directory, when the environment names one.
    pub home: Option<&'a Path>,
}

pub fn choose_database_path(input: &DatabasePathInput<'_>) -> Result<PathBuf> {
    if let Some(path) = non_empty(input.database_path_env.map(OsString::from)) {
        return Ok(path);
    }
    Ok(choose_data_root(&DataRootInput {
        data_root_env: input.data_root_env,
        home: input.home,
    })?
    .join(DATABASE_FILE))
}

/// Resolve the database file for the current environment.
pub fn resolve_database_path() -> Result<PathBuf> {
    let database_path_env: Option<OsString> = std::env::var_os(DB_PATH_ENV);
    let data_root_env: Option<OsString> = std::env::var_os(DATA_ROOT_ENV);
    let home = home_dir();
    choose_database_path(&DatabasePathInput {
        database_path_env: database_path_env.as_deref(),
        data_root_env: data_root_env.as_deref(),
        home: home.as_deref(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        choose_data_root, choose_database_path, DataRootInput, DatabasePathInput, UserPaths,
        DATABASE_FILE, DATA_ROOT_ENV, HOME_DIR_NAME, RUNTIME_LOG_FILE,
    };
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    const HOME: &str = "/home/dev";

    fn data_root(data_env: Option<&OsStr>, home: Option<&Path>) -> PathBuf {
        choose_data_root(&DataRootInput {
            data_root_env: data_env,
            home,
        })
        .unwrap()
    }

    #[test]
    fn explicit_data_root_beats_the_home_directory() {
        assert_eq!(
            data_root(Some(OsStr::new("/mnt/shared")), Some(Path::new(HOME))),
            PathBuf::from("/mnt/shared")
        );
    }

    #[test]
    fn defaults_to_the_user_directory_under_home() {
        assert_eq!(
            data_root(None, Some(Path::new(HOME))),
            PathBuf::from(HOME).join(HOME_DIR_NAME)
        );
    }

    #[test]
    fn empty_data_root_is_ignored_not_treated_as_a_path() {
        assert_eq!(
            data_root(Some(OsStr::new("")), Some(Path::new(HOME))),
            PathBuf::from(HOME).join(HOME_DIR_NAME)
        );
    }

    #[test]
    fn missing_home_errors_instead_of_guessing_a_write_target() {
        let error = choose_data_root(&DataRootInput {
            data_root_env: None,
            home: None,
        })
        .unwrap_err();
        assert!(
            error.to_string().contains(DATA_ROOT_ENV),
            "error must name the env override: {error}"
        );
    }

    #[test]
    fn explicit_database_file_wins_over_the_data_root() {
        let chosen = choose_database_path(&DatabasePathInput {
            database_path_env: Some(OsStr::new("/data/other.db")),
            data_root_env: Some(OsStr::new("/ignored")),
            home: Some(Path::new(HOME)),
        })
        .unwrap();
        assert_eq!(chosen, PathBuf::from("/data/other.db"));
    }

    #[test]
    fn database_follows_the_data_root_when_not_named() {
        let chosen = choose_database_path(&DatabasePathInput {
            database_path_env: None,
            data_root_env: Some(OsStr::new("/mnt/shared")),
            home: Some(Path::new(HOME)),
        })
        .unwrap();
        assert_eq!(chosen, PathBuf::from("/mnt/shared").join(DATABASE_FILE));
    }

    #[test]
    fn database_resolution_fails_without_any_home_or_override() {
        assert!(choose_database_path(&DatabasePathInput {
            database_path_env: None,
            data_root_env: None,
            home: None,
        })
        .is_err());
    }

    #[test]
    fn every_layout_path_stays_under_the_root() {
        let paths = UserPaths::under("/home/dev/.deepagent");
        let root = Path::new("/home/dev/.deepagent");
        let all = [
            paths.database.clone(),
            paths.runtime_logs.clone(),
            paths.files.clone(),
            paths.attachments.clone(),
            paths.recordings.clone(),
            paths.plugins.clone(),
            paths.plugin_state_root.clone(),
            paths.plugin_data.clone(),
            paths.skills.clone(),
            paths.skills_user.clone(),
            paths.skills_marketplace.clone(),
            paths.runtimes.clone(),
            paths.mcp.clone(),
            paths.cache.clone(),
            paths.vision_cache.clone(),
            paths.tool_results.clone(),
            paths.artifacts.clone(),
            paths.ssh.clone(),
            paths.retired.clone(),
            paths.install_ledger(),
        ];
        for path in &all {
            assert!(path.starts_with(root), "{path:?} escapes the data root");
        }
        for dir in paths.managed_dirs() {
            assert!(dir.starts_with(&paths.files));
        }
    }

    #[test]
    fn plugin_state_and_personal_plugin_directories_are_distinct() {
        let paths = UserPaths::under("/home/dev/.deepagent");
        assert_eq!(
            paths.database,
            PathBuf::from("/home/dev/.deepagent").join(DATABASE_FILE)
        );
        assert_eq!(paths.runtime_logs.file_name().unwrap(), RUNTIME_LOG_FILE);
        assert_eq!(paths.plugins, paths.files.join("plugins"));
        assert_eq!(paths.plugin_state_root, paths.root.join("plugins"));
        assert_eq!(paths.plugin_data, paths.root.join("plugins").join("data"));
        assert_eq!(
            paths.skills_marketplace,
            paths.files.join("skills").join("user").join("marketplace")
        );
    }

    #[test]
    fn ensure_dirs_creates_the_managed_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = UserPaths::under(tmp.path());
        paths.ensure_dirs().unwrap();
        for dir in paths.managed_dirs() {
            assert!(dir.is_dir(), "missing {}", dir.display());
        }
        assert!(!paths.retired.exists(), "retired is created only on demand");
    }
}
