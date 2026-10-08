//! Move data written by older builds into the user-directory data root.
//!
//! Before this rule existed the desktop shell kept everything under
//! `<install dir>/data` and, before that, under `%APPDATA%\com.deepagent.studio`.
//! Those locations sit inside the installer's footprint, so an uninstall either
//! destroys user data or leaves the old copy behind to be picked up again by
//! mistake. Migration therefore *moves* the tree: after a successful run the
//! legacy root no longer exists.
//!
//! The database gets a gate before it moves: this process must hold the legacy
//! root's host lock (another live host means we stop), then WAL is checkpointed
//! and `PRAGMA integrity_check` must answer `ok`. Nothing is copied as a
//! fallback — a half-moved root is reported, not papered over.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::Database;

use crate::data_root::{UserPaths, DATABASE_FILE};
use crate::host_lock;

/// What a host asked the data-root installer to do.
#[derive(Debug, Clone)]
pub struct DataRootRequest<'a> {
    /// The shell's platform app-data directory, when it has a precise value.
    /// Otherwise the platform rule in `data_root::legacy_app_data_dir` is used.
    pub app_data_dir: Option<&'a Path>,
    /// Extra roots to migrate ahead of the discovered ones (`--from`).
    pub extra_sources: &'a [PathBuf],
    /// Report the plan without touching anything.
    pub dry_run: bool,
}

/// Outcome of preparing a data root.
#[derive(Debug, Clone)]
pub struct DataRootPreparation {
    pub paths: UserPaths,
    pub reports: Vec<MigrationReport>,
    /// Layout entries re-placed into the current directory shape.
    pub folded: usize,
}

/// Resolve the user data root, move in whatever older builds left behind, and
/// create the managed directories. Every host calls this **before** opening the
/// database: creating `~/.deepagent/deepagent.db` first would collide with the
/// legacy database and block the very migration it was meant to replace.
pub fn install_data_root(request: &DataRootRequest<'_>) -> Result<DataRootPreparation> {
    let paths = UserPaths::resolve()?;
    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let legacy_app_data = request
        .app_data_dir
        .map(Path::to_path_buf)
        .or_else(crate::data_root::legacy_app_data_dir);
    let mut sources = discover_legacy_sources(
        install_dir.as_deref(),
        legacy_app_data.as_deref(),
        &paths.root,
    );
    for extra in request.extra_sources {
        sources.insert(0, extra.clone());
    }

    let reports = migrate_into(&paths, &sources, request.dry_run)?;
    if request.dry_run {
        return Ok(DataRootPreparation {
            paths,
            reports,
            folded: 0,
        });
    }

    let mut folded = normalize_user_root_layout(&paths)?.len();
    if let Some(install_dir) = install_dir.as_deref() {
        folded += fold_install_dir_layout(install_dir, &paths)?.len();
    }
    paths.ensure_dirs()?;
    Ok(DataRootPreparation {
        paths,
        reports,
        folded,
    })
}

/// What one legacy root contributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub source: PathBuf,
    pub target: PathBuf,
    /// Entries handled, relative to `source`. Directories come before the files
    /// inside them.
    pub moved: Vec<PathBuf>,
    /// True when the root was already the target or held no DeepAgent data.
    pub skipped: bool,
}

impl MigrationReport {
    fn skipped(source: PathBuf, target: PathBuf) -> Self {
        Self {
            source,
            target,
            moved: Vec::new(),
            skipped: true,
        }
    }
}

/// A directory only counts as ours when it holds a recognizable artifact, so a
/// stray `data` folder next to the executable is never moved by accident.
fn looks_like_data_root(source: &Path) -> bool {
    source.join(DATABASE_FILE).is_file()
        || source.join("files").is_dir()
        || source.join("plugins").is_dir()
}

/// Compare two paths after resolution; a path that cannot be resolved falls back
/// to a literal comparison so a missing directory never lands on another one.
pub fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => a == b,
    }
}

/// Roots that may still hold data from an older build, newest first: the install
/// directory's `data` folder, then the platform app-data directory.
pub fn discover_legacy_sources(
    install_dir: Option<&Path>,
    app_data_dir: Option<&Path>,
    target_root: &Path,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(install_dir) = install_dir {
        candidates.push(install_dir.join("data"));
    }
    candidates.extend(
        app_data_dir
            .filter(|path| !path.as_os_str().is_empty())
            .map(Path::to_path_buf),
    );
    candidates
        .into_iter()
        .filter(|source| !same_path(source, target_root))
        .filter(|source| looks_like_data_root(source))
        .collect()
}

/// Top-level names the post-move layout fold re-places. A cross-root move keeps
/// directory names as they were, so an old build's `tool_results` lands as
/// `<root>/tool_results`; `normalize_user_root_layout` then puts it where the
/// current layout reads it. Hosts report this list so a dry run is honest about
/// the second step.
pub fn layout_fold_sources() -> Vec<&'static str> {
    USER_ROOT_FOLDS.iter().map(|(name, _)| *name).collect()
}

/// Move every legacy root into `target`, discovering them from the running
/// executable and the shell's app-data directory.
///
/// With `dry_run` the function only reports what it *would* move and stops
/// before any side effect.
pub fn migrate_data_roots(
    target: &UserPaths,
    install_dir: Option<&Path>,
    app_data_dir: Option<&Path>,
    dry_run: bool,
) -> Result<Vec<MigrationReport>> {
    let sources = discover_legacy_sources(install_dir, app_data_dir, &target.root);
    migrate_into(target, &sources, dry_run)
}

/// Move the given legacy roots into `target`. Split out so tests never depend on
/// where the real executable lives.
pub fn migrate_into(
    target: &UserPaths,
    sources: &[PathBuf],
    dry_run: bool,
) -> Result<Vec<MigrationReport>> {
    let mut reports = Vec::new();
    for source in sources {
        reports.push(migrate_one(source, target, dry_run)?);
    }
    Ok(reports)
}

fn migrate_one(source: &Path, target: &UserPaths, dry_run: bool) -> Result<MigrationReport> {
    let source = source.to_path_buf();
    if same_path(&source, &target.root) || !looks_like_data_root(&source) {
        return Ok(MigrationReport::skipped(source, target.root.clone()));
    }

    let mut entries = Vec::new();
    collect_entries(&source, &source, &mut entries)?;
    if dry_run {
        return Ok(MigrationReport {
            moved: entries,
            skipped: false,
            source,
            target: target.root.clone(),
        });
    }

    let conflicts = find_conflicts(&source, &entries, &target.root);
    if !conflicts.is_empty() {
        let shown: Vec<String> = conflicts
            .iter()
            .take(8)
            .map(|path| path.display().to_string())
            .collect();
        return Err(CoreError::invalid(format!(
            "refusing to migrate '{}' into '{}': {} file(s) already exist at the target \
             ({}). Resolve them, then retry.",
            source.display(),
            target.root.display(),
            conflicts.len(),
            shown.join(", ")
        )));
    }

    checkpoint_database(&source)?;
    target.ensure_dirs()?;
    let mut moved = Vec::new();
    for relative in &entries {
        move_entry(&source.join(relative), &target.root.join(relative)).map_err(|error| {
            CoreError::Persistence(format!(
                "migrating {} -> {}: {error}",
                source.join(relative).display(),
                target.root.join(relative).display()
            ))
        })?;
        moved.push(relative.clone());
    }
    remove_if_emptied(&source);

    tracing::info!(
        source = %source.display(),
        target = %target.root.display(),
        entries = moved.len(),
        "migrated the legacy data root into the user data root"
    );
    Ok(MigrationReport {
        moved,
        skipped: false,
        source,
        target: target.root.clone(),
    })
}

/// Every entry under `dir`, as paths relative to `root`; directories are listed
/// before the files inside them.
fn collect_entries(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let read = std::fs::read_dir(dir)
        .map_err(|error| CoreError::Persistence(format!("read {}: {error}", dir.display())))?;
    for entry in read {
        let entry = entry.map_err(|error| {
            CoreError::Persistence(format!("read entry under {}: {error}", dir.display()))
        })?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .to_path_buf();
        let file_type = entry.file_type().map_err(|error| {
            CoreError::Persistence(format!("read type {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            out.push(relative.clone());
            collect_entries(root, &path, out)?;
        } else {
            out.push(relative);
        }
    }
    Ok(())
}

/// Target files that already exist. The database is always reported first
/// because two live databases is the one conflict only a human can resolve.
fn find_conflicts(source: &Path, entries: &[PathBuf], target_root: &Path) -> Vec<PathBuf> {
    let mut conflicts = BTreeSet::new();
    let database = source.join(DATABASE_FILE);
    if database.is_file() && target_root.join(DATABASE_FILE).exists() {
        conflicts.insert(target_root.join(DATABASE_FILE));
    }
    for relative in entries {
        let from = source.join(relative);
        let to = target_root.join(relative);
        if from.is_file() && to.exists() {
            conflicts.insert(to);
        }
    }
    conflicts.into_iter().collect()
}

/// Hold the legacy root, flush WAL, and require an intact database before the
/// file moves. A standby result means another host is live: stop, never half-move.
fn checkpoint_database(source: &Path) -> Result<()> {
    let database = source.join(DATABASE_FILE);
    if !database.is_file() {
        return Ok(());
    }
    if let host_lock::HostRole::Standby { holder } = host_lock::claim(source)? {
        host_lock::release(source);
        return Err(CoreError::other(format!(
            "another host owns '{}' (holder: {holder}); close it before migrating",
            source.display()
        )));
    }
    let result = (|| -> Result<()> {
        let db = Database::open(&database).map_err(|error| {
            CoreError::Persistence(format!(
                "open {} for migration: {error}",
                database.display()
            ))
        })?;
        db.with_conn(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(|error| CoreError::Persistence(format!("WAL checkpoint: {error}")))?;
            let verdict: String = conn
                .query_row("PRAGMA integrity_check;", [], |row| row.get(0))
                .map_err(|error| CoreError::Persistence(format!("integrity check: {error}")))?;
            if verdict == "ok" {
                Ok(())
            } else {
                Err(CoreError::Persistence(format!(
                    "integrity_check reported {verdict:?} for {}",
                    database.display()
                )))
            }
        })
    })();
    host_lock::release(source);
    result
}

/// Directories are recreated at the target and pruned from the source later;
/// files move by rename, degrading to copy-then-delete across volumes.
fn move_entry(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        return std::fs::create_dir_all(to)
            .map_err(|error| CoreError::Persistence(format!("create {}: {error}", to.display())));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CoreError::Persistence(format!("create {}: {error}", parent.display()))
        })?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|error| {
        CoreError::Persistence(format!(
            "copy {} -> {}: {error}",
            from.display(),
            to.display()
        ))
    })?;
    std::fs::remove_file(from).map_err(|error| {
        CoreError::Persistence(format!(
            "copied {} to {} but could not remove the original: {error}",
            from.display(),
            to.display()
        ))
    })
}

/// Compare two files byte-for-byte without loading either one whole.
fn identical_files(a: &Path, b: &Path) -> Result<bool> {
    let stat = |path: &Path| -> Result<u64> {
        std::fs::metadata(path)
            .map(|metadata| metadata.len())
            .map_err(|error| CoreError::Persistence(format!("stat {}: {error}", path.display())))
    };
    if stat(a)? != stat(b)? {
        return Ok(false);
    }
    const CHUNK: usize = 64 * 1024;
    let mut left = std::fs::File::open(a)
        .map_err(|error| CoreError::Persistence(format!("open {}: {error}", a.display())))?;
    let mut right = std::fs::File::open(b)
        .map_err(|error| CoreError::Persistence(format!("open {}: {error}", b.display())))?;
    let mut buffer_left = vec![0u8; CHUNK];
    let mut buffer_right = vec![0u8; CHUNK];
    loop {
        let read_left = fill(&mut left, &mut buffer_left)?;
        let read_right = fill(&mut right, &mut buffer_right)?;
        if read_left != read_right || buffer_left[..read_left] != buffer_right[..read_right] {
            return Ok(false);
        }
        if read_left == 0 {
            return Ok(true);
        }
    }
}

/// Read until `buffer` is full or the file ends, returning the filled length.
fn fill(file: &mut std::fs::File, buffer: &mut [u8]) -> Result<usize> {
    use std::io::Read;
    let mut filled = 0usize;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(CoreError::Persistence(format!("read file: {error}"))),
        }
    }
    Ok(filled)
}

/// Delete the legacy root once it holds nothing, bottom-up. Anything left behind
/// means a failed move and must stay visible for the next run.
fn remove_if_emptied(source: &Path) {
    // The migration itself claims the lock, so these two artifacts are always
    // created after the entry list was taken and would otherwise keep the
    // legacy root alive forever.
    drop(remove_lock_artifacts(source));
    prune_empty_dirs(source);
    match std::fs::remove_dir(source) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            source = %source.display(),
            %error,
            "legacy data root was not fully emptied; keeping it for the next migration"
        ),
    }
}

fn remove_lock_artifacts(source: &Path) -> std::io::Result<()> {
    for artifact in [crate::host_lock::LOCK_FILE, crate::host_lock::OWNER_FILE] {
        let path = source.join(artifact);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// A legacy directory and where its contents belong in the current layout, as
/// path segments under the data root. `plugins` is deliberately absent from the
/// in-root table: that name now holds plugin *state*, and its decommissioned
/// marketplace children are reclaimed by `PluginService` itself.
const USER_ROOT_FOLDS: [(&str, &[&str]); 7] = [
    ("skills", &["files", "skills", "user"]),
    ("runtimes", &["files", "runtimes"]),
    ("mcp-data", &["files", "mcp"]),
    ("attachments", &["files", "attachments"]),
    ("recordings", &["files", "recordings"]),
    ("tool_results", &["files", "tool-results"]),
    ("vision-cache", &["files", "cache", "vision"]),
];

/// What an old install directory holds that is *not* a data root: the personal
/// plugin collections and the downloaded runtimes it wrote next to the
/// executable. Bundled resources (`resources/plugins`, `resources/runtimes`) are
/// installer content and stay where they are.
const INSTALL_DIR_FOLDS: [(&str, &[&str]); 2] = [
    ("plugins", &["files", "plugins"]),
    ("runtimes", &["files", "runtimes"]),
];

/// Fold the pre-`files/` layout inside the data root into the current one, so
/// skills and runtimes written by an old build stay reachable instead of becoming
/// orphans the new layout cannot see.
pub fn normalize_user_root_layout(target: &UserPaths) -> Result<Vec<PathBuf>> {
    fold_layout(target.root.as_path(), &USER_ROOT_FOLDS, &target.root)
}

/// Fold the user assets an old build wrote next to the executable into the data
/// root. Without this, plugins installed before the move would silently vanish.
pub fn fold_install_dir_layout(install_dir: &Path, target: &UserPaths) -> Result<Vec<PathBuf>> {
    if same_path(install_dir, &target.root) {
        return Ok(Vec::new());
    }
    fold_layout(install_dir, &INSTALL_DIR_FOLDS, &target.root)
}

fn fold_layout(base: &Path, folds: &[(&str, &[&str])], target_root: &Path) -> Result<Vec<PathBuf>> {
    let mut moved = Vec::new();
    for (legacy, segments) in folds {
        let from = base.join(legacy);
        let mut to = target_root.to_path_buf();
        for segment in segments.iter() {
            to = to.join(segment);
        }
        if !from.is_dir() || same_path(&from, &to) || to.starts_with(&from) {
            continue;
        }
        let mut entries = Vec::new();
        collect_entries(&from, &from, &mut entries)?;
        // A byte-identical duplicate of the canonical file is dropped rather than
        // merged: an old build wrote the same content into two places, and keeping
        // both would leave the uninstaller chasing two trees for one asset.
        // Anything that actually differs still stops the fold.
        let mut planned = Vec::new();
        let mut duplicates = Vec::new();
        let mut conflicts = Vec::new();
        for relative in entries {
            let source = from.join(&relative);
            let target = to.join(&relative);
            if source.is_file() && target.exists() {
                if identical_files(&source, &target)? {
                    duplicates.push(source);
                } else {
                    conflicts.push(target);
                }
            } else {
                planned.push(relative);
            }
        }
        if !conflicts.is_empty() {
            let shown: Vec<String> = conflicts
                .iter()
                .take(8)
                .map(|path| path.display().to_string())
                .collect();
            return Err(CoreError::invalid(format!(
                "refusing to fold '{}' into '{}': {} file(s) differ at the target ({}) and \
                 cannot be replaced automatically. Resolve them, then retry.",
                from.display(),
                to.display(),
                conflicts.len(),
                shown.join(", ")
            )));
        }
        for relative in &planned {
            move_entry(&from.join(relative), &to.join(relative))?;
            moved.push(relative.clone());
        }
        for duplicate in &duplicates {
            std::fs::remove_file(duplicate).map_err(|error| {
                CoreError::Persistence(format!("remove duplicate {}: {error}", duplicate.display()))
            })?;
        }
        prune_empty_dirs(&from);
        if let Err(error) = std::fs::remove_dir(&from) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %from.display(), %error, "could not remove the folded legacy directory");
            }
        }
    }
    if !moved.is_empty() {
        tracing::info!(
            base = %base.display(),
            entries = moved.len(),
            "folded the legacy layout into the current one"
        );
    }
    Ok(moved)
}

fn prune_empty_dirs(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            prune_empty_dirs(&path);
        }
    }
    let _ = std::fs::read_dir(dir).map(|mut read| {
        if read.next().is_none() {
            let _ = std::fs::remove_dir(dir);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        collect_entries, discover_legacy_sources, fold_install_dir_layout, migrate_data_roots,
        migrate_into, normalize_user_root_layout, same_path,
    };
    use crate::data_root::UserPaths;
    use deepagent_core::CoreError;
    use deepagent_persistence::Database;
    use std::path::{Path, PathBuf};

    fn persist_error(error: rusqlite::Error) -> CoreError {
        CoreError::Persistence(error.to_string())
    }

    fn file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn rel(value: &str) -> PathBuf {
        PathBuf::from(value)
    }

    #[test]
    fn discovers_only_roots_that_look_like_our_data() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("install");
        file(&install.join("data").join("deepagent.db"), "x");
        let app_data = tmp.path().join("roaming");
        file(&app_data.join("files").join("seed.txt"), "x");
        let stray = tmp.path().join("stray");
        file(&stray.join("data").join("readme.txt"), "x");

        let target = tmp.path().join("home").join(".deepagent");
        let found = discover_legacy_sources(Some(&stray), Some(&app_data), &target);
        assert_eq!(found, vec![app_data.clone()]);
        let found = discover_legacy_sources(Some(&install), Some(&app_data), &target);
        assert_eq!(
            found,
            vec![install.join("data"), app_data],
            "the install root is newer and comes first"
        );
    }

    #[test]
    fn never_lists_the_current_target_root() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("install");
        file(&install.join("data").join("deepagent.db"), "x");
        let target = install.join("data");
        assert!(discover_legacy_sources(Some(&install), None, &target).is_empty());
    }

    #[test]
    fn moves_the_whole_tree_and_removes_the_legacy_root() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("install").join("data");
        file(
            &legacy
                .join("files")
                .join("plugins")
                .join("demo")
                .join("plugin.json"),
            "{}",
        );
        file(
            &legacy
                .join("files")
                .join("skills")
                .join("user")
                .join("s.txt"),
            "s",
        );
        file(
            &legacy.join("plugins").join("state.json"),
            "{\"version\":1}",
        );
        file(&legacy.join("runtime-logs.db"), "logs");
        let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));

        let reports = migrate_into(&target, std::slice::from_ref(&legacy), false).unwrap();
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].skipped);
        assert!(target.plugins.join("demo").join("plugin.json").is_file());
        assert!(target.skills_user.join("s.txt").is_file());
        assert!(target.root.join("plugins").join("state.json").is_file());
        assert!(target.runtime_logs.is_file());
        assert!(
            !legacy.exists(),
            "the legacy root must be gone after a move"
        );
    }

    #[test]
    fn dry_run_reports_the_plan_and_touches_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("install").join("data");
        file(&legacy.join("files").join("mcp").join("a.json"), "{}");
        let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));

        let reports = migrate_into(&target, std::slice::from_ref(&legacy), true).unwrap();
        assert_eq!(
            reports[0].moved,
            vec![rel("files"), rel("files/mcp"), rel("files/mcp/a.json")]
        );
        assert!(
            !target.root.join("files").join("mcp").exists(),
            "no directories created"
        );
        assert!(legacy.exists(), "a dry run must leave the source untouched");
    }

    #[test]
    fn colliding_files_abort_before_anything_moves() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("install").join("data");
        file(
            &legacy.join("files").join("plugins").join("dup.txt"),
            "legacy",
        );
        file(
            &legacy.join("files").join("plugins").join("new.txt"),
            "legacy",
        );
        let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));
        file(&target.plugins.join("dup.txt"), "current");

        let error = migrate_into(&target, std::slice::from_ref(&legacy), false).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("already exist"), "{message}");
        assert!(message.contains("dup.txt"), "{message}");
        assert!(
            legacy
                .join("files")
                .join("plugins")
                .join("new.txt")
                .is_file(),
            "an aborted migration must leave both sides intact"
        );
        assert_eq!(
            std::fs::read_to_string(target.plugins.join("dup.txt")).unwrap(),
            "current"
        );
    }

    #[test]
    fn a_real_database_moves_and_stays_readable() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("install").join("data");
        let db = Database::open(legacy.join("deepagent.db")).unwrap();
        db.with_conn(|conn| {
            conn.execute_batch("CREATE TABLE probe(v TEXT); INSERT INTO probe VALUES('kept');")
                .map_err(persist_error)
        })
        .unwrap();
        drop(db);
        let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));

        migrate_into(&target, std::slice::from_ref(&legacy), false).unwrap();

        let moved = Database::open(&target.database).unwrap();
        let value: String = moved
            .with_conn(|conn| {
                conn.query_row("SELECT v FROM probe", [], |row| row.get(0))
                    .map_err(persist_error)
            })
            .unwrap();
        assert_eq!(value, "kept");
        assert!(!legacy.join("deepagent.db").exists());
        assert!(
            !legacy.exists(),
            "a migrated root is removed, lock artifacts included"
        );
    }

    #[test]
    fn a_corrupt_database_blocks_the_move() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("install").join("data");
        file(&legacy.join("deepagent.db"), "not a sqlite file at all");
        file(&legacy.join("files").join("keep.txt"), "keep");
        let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));

        let error = migrate_into(&target, std::slice::from_ref(&legacy), false).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("deepagent.db"),
            "the refusal must name the file: {message}"
        );
        assert!(
            legacy.join("files").join("keep.txt").is_file(),
            "nothing may move once the database fails the gate"
        );
        assert!(!target.database.exists());
    }

    #[test]
    fn install_dir_assets_land_in_the_managed_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("install");
        file(
            &install.join("plugins").join("helper").join("plugin.json"),
            "{}",
        );
        file(
            &install.join("runtimes").join("pandoc").join("bin.txt"),
            "p",
        );
        // Installer content must never move.
        file(
            &install
                .join("resources")
                .join("plugins")
                .join("builtin")
                .join("plugin.json"),
            "{}",
        );
        let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));

        let moved = fold_install_dir_layout(&install, &target).unwrap();
        assert!(!moved.is_empty());
        assert!(target.plugins.join("helper").join("plugin.json").is_file());
        assert!(target.runtimes.join("pandoc").join("bin.txt").is_file());
        assert!(install
            .join("resources")
            .join("plugins")
            .join("builtin")
            .join("plugin.json")
            .is_file());
        assert!(!install.join("plugins").exists());
    }

    #[test]
    fn folds_the_pre_files_layout_into_the_current_one() {
        let tmp = tempfile::tempdir().unwrap();
        let target = UserPaths::under(tmp.path());
        target.ensure_dirs().unwrap();
        file(
            &target
                .root
                .join("skills")
                .join("marketplace")
                .join("pdf")
                .join("SKILL.md"),
            "v1",
        );
        file(
            &target.root.join("runtimes").join("pandoc").join("bin.txt"),
            "p",
        );

        let moved = normalize_user_root_layout(&target).unwrap();
        assert!(moved.contains(&rel("marketplace")), "{moved:?}");
        assert!(target
            .skills_marketplace
            .join("pdf")
            .join("SKILL.md")
            .is_file());
        assert!(target.runtimes.join("pandoc").join("bin.txt").is_file());
        assert!(
            !target.root.join("skills").exists(),
            "the old directory is folded away"
        );
        assert!(!target.root.join("runtimes").exists());
    }

    #[test]
    fn a_fold_refuses_when_a_name_collides_with_different_content() {
        let tmp = tempfile::tempdir().unwrap();
        let target = UserPaths::under(tmp.path());
        target.ensure_dirs().unwrap();
        file(&target.root.join("skills").join("a.txt"), "legacy");
        file(&target.skills_user.join("a.txt"), "current");

        let error = normalize_user_root_layout(&target).unwrap_err();
        assert!(error.to_string().contains("fold"), "{error}");
        assert!(
            target.root.join("skills").join("a.txt").is_file(),
            "nothing moves"
        );
        assert_eq!(
            std::fs::read_to_string(target.skills_user.join("a.txt")).unwrap(),
            "current"
        );
    }

    #[test]
    fn a_fold_drops_byte_identical_duplicates_instead_of_refusing() {
        let tmp = tempfile::tempdir().unwrap();
        let target = UserPaths::under(tmp.path());
        target.ensure_dirs().unwrap();
        // An old build wrote the same vision cache into two places.
        file(
            &target.root.join("vision-cache").join("h1.json"),
            "{\"a\":1}",
        );
        file(
            &target.root.join("vision-cache").join("h2.json"),
            "{\"a\":2}",
        );
        file(&target.vision_cache.join("h1.json"), "{\"a\":1}");

        let moved = normalize_user_root_layout(&target).unwrap();
        assert_eq!(moved, vec![rel("h2.json")], "only the missing copy moves");
        assert_eq!(
            std::fs::read_to_string(target.vision_cache.join("h1.json")).unwrap(),
            "{\"a\":1}"
        );
        assert!(
            !target.root.join("vision-cache").exists(),
            "the stale directory is gone once its only copies are resolved"
        );
    }

    #[test]
    fn roots_without_deepagent_data_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("install").join("data");
        file(&legacy.join("something-else.txt"), "x");
        let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));
        let reports = migrate_into(&target, std::slice::from_ref(&legacy), false).unwrap();
        assert!(reports[0].skipped);
        assert!(legacy.exists());
    }

    #[test]
    fn collect_lists_directories_before_their_files() {
        let tmp = tempfile::tempdir().unwrap();
        file(&tmp.path().join("files").join("x.txt"), "x");
        let mut out = Vec::new();
        collect_entries(tmp.path(), tmp.path(), &mut out).unwrap();
        assert_eq!(out, vec![rel("files"), rel("files/x.txt")]);
    }

    #[test]
    fn same_path_matches_a_dotted_spelling_and_rejects_missing_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(same_path(tmp.path(), &tmp.path().join(".")));
        assert!(!same_path(tmp.path(), &tmp.path().join("nope")));
    }

    #[test]
    fn discovery_from_the_real_executable_never_fakes_a_legacy_root() {
        let tmp = tempfile::tempdir().unwrap();
        let target = UserPaths::under(tmp.path().join("home"));
        let reports = migrate_data_roots(&target, None, None, true).unwrap();
        assert!(
            reports.is_empty(),
            "nothing to discover without legacy roots"
        );
    }
}
