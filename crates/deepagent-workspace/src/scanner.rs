//! The workspace scanner (开发计划.md Phase 3 §2).
//!
//! Walks the project directory (bounded by depth / entry limits), skips noisy
//! directories (`target`, `node_modules`, `.git`, …), detects the project
//! ecosystem(s) from manifests, and extracts compact summaries — producing a
//! [`WorkspaceSnapshot`].
//!
//! The walk is intentionally synchronous and dependency-free: it is fast for
//! typical repos and easy to reason about. Heavy/remote scanning can be layered
//! on later behind the same snapshot output.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};

use crate::snapshot::{ManifestInfo, ProjectKind, WorkspaceSnapshot};

/// Limits that bound a scan so it stays cheap and the output stays within a
/// token budget.
#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    /// Maximum directory depth to descend.
    pub max_depth: usize,
    /// Maximum number of tree entries to render.
    pub max_entries: usize,
    /// Maximum bytes of README to excerpt.
    pub readme_bytes: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_depth: 4,
            max_entries: 200,
            readme_bytes: 1500,
        }
    }
}

/// Directory names skipped during the walk (build output, VCS, deps, caches).
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    ".cache",
    ".next",
    "vendor",
];

/// The scanner.
#[derive(Debug, Clone)]
pub struct WorkspaceScanner {
    limits: ScanLimits,
}

impl Default for WorkspaceScanner {
    fn default() -> Self {
        Self::new(ScanLimits::default())
    }
}

impl WorkspaceScanner {
    /// Build a scanner with the given limits.
    pub fn new(limits: ScanLimits) -> Self {
        Self { limits }
    }

    /// Scan `root` into a [`WorkspaceSnapshot`].
    pub fn scan(&self, root: impl AsRef<Path>) -> Result<WorkspaceSnapshot> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(CoreError::invalid(format!(
                "workspace root is not a directory: {}",
                root.display()
            )));
        }

        let mut state = WalkState::default();
        let mut tree = String::new();
        self.walk(root, root, 0, &mut tree, &mut state)?;

        // Detect kinds & summarize manifests from the discovered files.
        let mut kinds: BTreeSet<ProjectKind> = BTreeSet::new();
        let mut manifests = Vec::new();
        for rel in &state.manifest_paths {
            if let Some((kind, info)) = self.classify_manifest(root, rel) {
                if let Some(k) = kind {
                    kinds.insert(k);
                }
                manifests.push(info);
            }
        }

        let readme_excerpt = state
            .readme_path
            .as_ref()
            .and_then(|p| self.read_excerpt(root, p));

        Ok(WorkspaceSnapshot {
            root: root.display().to_string(),
            kinds: kinds.into_iter().collect(),
            tree: tree.trim_end().to_string(),
            manifests,
            readme_excerpt,
            file_count: state.file_count,
            dir_count: state.dir_count,
            truncated: state.truncated,
        })
    }

    fn walk(
        &self,
        root: &Path,
        dir: &Path,
        depth: usize,
        tree: &mut String,
        state: &mut WalkState,
    ) -> Result<()> {
        if depth >= self.limits.max_depth || state.entries >= self.limits.max_entries {
            if state.entries >= self.limits.max_entries {
                state.truncated = true;
            }
            return Ok(());
        }

        let mut entries: Vec<(PathBuf, bool)> = match fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|entry| {
                    let path = entry.ok()?.path();
                    let is_dir = entry_is_dir(&path);
                    Some((path, is_dir))
                })
                .collect(),
            Err(_) => return Ok(()), // unreadable dir: skip, don't fail the scan
        };
        // Deterministic order: directories first, then files, alphabetical.
        entries
            .sort_by(|(a, ad), (b, bd)| bd.cmp(ad).then_with(|| a.file_name().cmp(&b.file_name())));

        for (path, is_dir) in entries {
            if state.entries >= self.limits.max_entries {
                state.truncated = true;
                break;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            if is_dir {
                if IGNORED_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                    continue;
                }
                state.dir_count += 1;
                state.entries += 1;
                tree.push_str(&format!("{}{}/\n", indent(depth), name));
                self.walk(root, &path, depth + 1, tree, state)?;
            } else {
                state.file_count += 1;
                state.entries += 1;
                tree.push_str(&format!("{}{}\n", indent(depth), name));
                self.note_special_file(root, &path, &name, state);
            }
        }
        Ok(())
    }

    fn note_special_file(&self, root: &Path, path: &Path, name: &str, state: &mut WalkState) {
        let rel = relative(root, path);
        let lower = name.to_ascii_lowercase();
        if lower == "readme.md" || lower == "readme" || lower == "readme.txt" {
            // Prefer the shallowest README (root-level).
            if state.readme_path.is_none() {
                state.readme_path = Some(rel.clone());
            }
        }
        if matches!(
            name,
            "Cargo.toml"
                | "package.json"
                | "pyproject.toml"
                | "requirements.txt"
                | "setup.py"
                | "go.mod"
                | "pom.xml"
                | "build.gradle"
        ) {
            state.manifest_paths.push(rel);
        }
    }

    fn classify_manifest(
        &self,
        root: &Path,
        rel: &str,
    ) -> Option<(Option<ProjectKind>, ManifestInfo)> {
        let full = root.join(rel);
        let content = fs::read_to_string(&full).ok()?;
        let file = rel.rsplit('/').next().unwrap_or(rel);

        let (kind, summary) = match file {
            "Cargo.toml" => (
                Some(ProjectKind::Rust),
                summarize_toml_name(&content, "package"),
            ),
            "package.json" => (Some(ProjectKind::Node), summarize_package_json(&content)),
            "pyproject.toml" => (
                Some(ProjectKind::Python),
                summarize_toml_name(&content, "project"),
            ),
            "requirements.txt" | "setup.py" => (
                Some(ProjectKind::Python),
                first_nonempty_line(&content).unwrap_or_else(|| "python project".into()),
            ),
            "go.mod" => (
                Some(ProjectKind::Go),
                first_nonempty_line(&content).unwrap_or_else(|| "go module".into()),
            ),
            "pom.xml" | "build.gradle" => (Some(ProjectKind::Java), "java project".to_string()),
            _ => (None, "manifest".to_string()),
        };

        Some((
            kind,
            ManifestInfo {
                path: rel.to_string(),
                summary,
            },
        ))
    }

    fn read_excerpt(&self, root: &Path, rel: &str) -> Option<String> {
        let content = fs::read_to_string(root.join(rel)).ok()?;
        let mut excerpt: String = content.chars().take(self.limits.readme_bytes).collect();
        if content.len() > excerpt.len() {
            excerpt.push_str("\n…");
        }
        Some(excerpt.trim().to_string())
    }
}

fn entry_is_dir(path: &Path) -> bool {
    #[cfg(test)]
    tests::DIRECTORY_PROBES.with(|calls| calls.set(calls.get() + 1));
    path.is_dir()
}

#[derive(Default)]
struct WalkState {
    entries: usize,
    file_count: usize,
    dir_count: usize,
    truncated: bool,
    readme_path: Option<String>,
    manifest_paths: Vec<String>,
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn first_nonempty_line(content: &str) -> Option<String> {
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(120).collect())
}

/// Extract a `name = "..."` under a `[section]` table from minimal TOML.
fn summarize_toml_name(content: &str, section: &str) -> String {
    let mut in_section = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == format!("[{section}]");
            continue;
        }
        if in_section {
            if let Some(rest) = t.strip_prefix("name") {
                if let Some(eq) = rest.trim().strip_prefix('=') {
                    let name = eq.trim().trim_matches('"');
                    return format!("name = {name}");
                }
            }
        }
    }
    format!("{section} manifest")
}

/// Extract `name` + dependency count from a package.json.
fn summarize_package_json(content: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return "package.json".to_string(),
    };
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("(unnamed)");
    let deps = value
        .get("dependencies")
        .and_then(|v| v.as_object())
        .map(|o| o.len())
        .unwrap_or(0);
    format!("name = {name}, deps = {deps}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    thread_local! {
        pub(super) static DIRECTORY_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn classifies_each_entry_once_without_changing_sorted_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..16 {
            write(dir.path(), &format!("dir_{index:02}/child.txt"), "child");
        }
        for index in 0..32 {
            write(dir.path(), &format!("file_{index:02}.txt"), "file");
        }
        write(dir.path(), "target/debug/ignored.txt", "ignored");
        write(dir.path(), ".hidden/ignored.txt", "ignored");
        let scanner = WorkspaceScanner::new(ScanLimits {
            max_depth: 1,
            ..Default::default()
        });
        DIRECTORY_PROBES.with(|calls| calls.set(0));
        let snapshot = scanner.scan(dir.path()).unwrap();
        let expected = (0..16)
            .map(|index| format!("dir_{index:02}/"))
            .chain((0..32).map(|index| format!("file_{index:02}.txt")))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(snapshot.tree, expected);
        assert_eq!((snapshot.dir_count, snapshot.file_count), (16, 32));
        assert!(!snapshot.truncated);
        assert_eq!(
            DIRECTORY_PROBES.with(|calls| calls.get()),
            50,
            "directory classification must not be repeated by sorting or rendering"
        );
    }

    #[test]
    fn subsequent_scan_observes_file_replaced_by_directory_and_updated_manifest() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "entry", "before");
        write(dir.path(), "Cargo.toml", "[package]\nname = \"before\"");
        let scanner = WorkspaceScanner::default();
        let before = scanner.scan(dir.path()).unwrap();
        fs::remove_file(dir.path().join("entry")).unwrap();
        write(dir.path(), "entry/child.txt", "after");
        write(dir.path(), "Cargo.toml", "[package]\nname = \"after\"");
        let after = scanner.scan(dir.path()).unwrap();
        assert_eq!(before.dir_count, 0);
        assert_eq!(after.dir_count, 1);
        assert!(after.tree.contains("entry/\n  child.txt"));
        assert_eq!(before.manifests[0].summary, "name = before");
        assert_eq!(after.manifests[0].summary, "name = after");
    }

    #[test]
    #[ignore = "explicit local scan benchmark; no timing assertion"]
    fn directory_scan_latency_probe() {
        let fixture = tempfile::tempdir().unwrap();
        for directory in 0..16 {
            for file in 0..12 {
                write(
                    fixture.path(),
                    &format!("dir_{directory:02}/file_{file:02}.txt"),
                    "synthetic",
                );
            }
        }
        for file in 0..256 {
            write(fixture.path(), &format!("file_{file:03}.txt"), "synthetic");
        }
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (name, root) in [
            ("wide_fixture", fixture.path()),
            ("repository", &repository),
        ] {
            let scanner = WorkspaceScanner::default();
            DIRECTORY_PROBES.with(|calls| calls.set(0));
            let started = std::time::Instant::now();
            let mut rendered_bytes = 0;
            for _ in 0..20 {
                let snapshot = scanner.scan(root).unwrap();
                rendered_bytes += snapshot.to_context_block().len();
            }
            println!(
                "scan_probe: source={name} calls=20 directory_probes={} elapsed_us={} rendered_bytes={rendered_bytes}",
                DIRECTORY_PROBES.with(|calls| calls.get()),
                started.elapsed().as_micros(),
            );
        }
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn scans_rust_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"",
        );
        write(root, "README.md", "# Demo\nA test project.");
        write(root, "src/main.rs", "fn main() {}");

        let snap = WorkspaceScanner::default().scan(root).unwrap();
        assert!(snap.kinds.contains(&ProjectKind::Rust));
        assert!(snap.tree.contains("src/"));
        assert!(snap.tree.contains("main.rs"));
        assert_eq!(
            snap.manifests
                .iter()
                .find(|m| m.path == "Cargo.toml")
                .unwrap()
                .summary,
            "name = demo"
        );
        assert!(snap.readme_excerpt.unwrap().contains("Demo"));
    }

    #[test]
    fn detects_node_project_and_deps() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "package.json",
            r#"{"name": "web", "dependencies": {"react": "18", "vite": "5"}}"#,
        );
        let snap = WorkspaceScanner::default().scan(root).unwrap();
        assert!(snap.kinds.contains(&ProjectKind::Node));
        let m = snap
            .manifests
            .iter()
            .find(|m| m.path == "package.json")
            .unwrap();
        assert!(m.summary.contains("name = web"));
        assert!(m.summary.contains("deps = 2"));
    }

    #[test]
    fn ignores_noise_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "src/lib.rs", "");
        write(root, "target/debug/junk.o", "");
        write(root, "node_modules/pkg/index.js", "");

        let snap = WorkspaceScanner::default().scan(root).unwrap();
        assert!(!snap.tree.contains("target/"));
        assert!(!snap.tree.contains("node_modules/"));
        assert!(snap.tree.contains("src/"));
    }

    #[test]
    fn polyglot_detects_multiple_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"x\"");
        write(root, "go.mod", "module example.com/x");
        let snap = WorkspaceScanner::default().scan(root).unwrap();
        assert!(snap.kinds.contains(&ProjectKind::Rust));
        assert!(snap.kinds.contains(&ProjectKind::Go));
    }

    #[test]
    fn respects_entry_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..50 {
            write(root, &format!("file_{i}.txt"), "x");
        }
        let scanner = WorkspaceScanner::new(ScanLimits {
            max_entries: 10,
            ..Default::default()
        });
        let snap = scanner.scan(root).unwrap();
        assert!(snap.truncated);
    }

    #[test]
    fn errors_on_nonexistent_root() {
        let err = WorkspaceScanner::default()
            .scan("/no/such/dir/here")
            .unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)));
    }
}
