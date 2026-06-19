//! `FileScanner`: recursive scan, filtering, language detection, content hash.
//!
//! The scanner walks a project tree, applies a set of filters (skip-listed
//! directories, binary file extensions, a size ceiling, and `.gitignore`
//! rules), classifies each surviving file's [`Language`], and computes a
//! BLAKE3 content hash used later for incremental change detection.
//!
//! See `design.md` (Components → FileScanner) for the contract. A single
//! file failing to read or stat is skipped with a `tracing::warn` rather than
//! aborting the whole scan.

use std::path::{Component, Path, PathBuf};

use deepagent_core::{CoreError, Result};
use ignore::WalkBuilder;

use crate::types::Language;

/// Files larger than this (in bytes) are skipped. 1.5 MiB.
const MAX_FILE_SIZE: u64 = 1_572_864;

/// Directory names that are never descended into.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    // Generated engine artifacts (the code-graph database and the projected
    // UA knowledge-graph.json live here). Skipping them keeps the engine's own
    // output from being scanned as project source and churning incremental
    // syncs.
    ".understand-anything",
    ".codegraph",
    ".deepagent",
];

/// File extensions (lower-case, without the dot) treated as binary / non-source
/// and therefore skipped.
const BINARY_EXTENSIONS: &[&str] = &[
    // images
    "png", "jpg", "jpeg", "gif", "ico", "svg", "webp", "bmp", "tiff", // fonts
    "woff", "woff2", "ttf", "eot", "otf", // native artifacts
    "exe", "dll", "so", "dylib", "o", "a", "lib", "obj", "bin", "wasm", "class", "jar",
    // archives
    "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", // documents
    "pdf", // audio / video
    "mp4", "mp3", "wav", "avi", "mov", "mkv", "flac", "ogg", "webm",
];

/// A file discovered by the scanner that survived all filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Path relative to the project root, in POSIX (`/`) form.
    pub relative_path: PathBuf,
    /// Detected source language ([`Language::Other`] for unrecognised files,
    /// which are still registered so they appear in the project map).
    pub language: Language,
    /// File size in bytes.
    pub size: u64,
    /// BLAKE3 hash of the file contents, as a lower-case hex string.
    pub content_hash: String,
}

/// Recursive project scanner with `.gitignore` awareness.
#[derive(Debug, Clone)]
pub struct FileScanner {
    /// Canonical project root used to compute relative paths.
    project_root: PathBuf,
}

impl FileScanner {
    /// Create a scanner rooted at `project_root`.
    ///
    /// The root is canonicalised so that emitted [`ScannedFile::path`] values
    /// are absolute. Returns [`CoreError::Invalid`] if the root cannot be
    /// resolved (e.g. it does not exist).
    pub fn new(project_root: &Path) -> Result<Self> {
        let canonical = project_root.canonicalize().map_err(|e| {
            CoreError::invalid(format!(
                "cannot resolve project root {}: {e}",
                project_root.display()
            ))
        })?;
        Ok(Self {
            project_root: canonical,
        })
    }

    /// Recursively scan `project_root`, returning every file that passes the
    /// filters (skip-listed dirs, binary extensions, size ceiling,
    /// `.gitignore`).
    ///
    /// `project_root` is canonicalised; it is normally the same root passed to
    /// [`FileScanner::new`]. Per-file IO failures are logged and skipped so a
    /// single unreadable file never aborts the scan.
    pub fn scan(&self, project_root: &Path) -> Result<Vec<ScannedFile>> {
        let walk_root = project_root.canonicalize().map_err(|e| {
            CoreError::invalid(format!(
                "cannot resolve project root {}: {e}",
                project_root.display()
            ))
        })?;

        // Relative paths are always expressed against the configured root so
        // emitted paths stay stable regardless of which subtree is walked.
        let relative_base = self.project_root.as_path();

        let walker = WalkBuilder::new(&walk_root)
            // Include dotfiles/dotdirs; the SKIP_DIRS list prunes the ones we
            // never want (.git, .venv) explicitly.
            .hidden(false)
            // Honour .gitignore files even when there is no .git directory
            // (require_git(false)); ignore ancestor gitignores for determinism.
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .require_git(false)
            .parents(false)
            .filter_entry(|entry| {
                // Prune skip-listed directories (and their whole subtree).
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    if let Some(name) = entry.file_name().to_str() {
                        return !SKIP_DIRS.contains(&name);
                    }
                }
                true
            })
            .build();

        let mut files = Vec::new();
        for result in walker {
            let entry = match result {
                Ok(entry) => entry,
                Err(err) => {
                    tracing::warn!(error = %err, "skipping entry the walker could not read");
                    continue;
                }
            };

            // Only regular files become ScannedFiles.
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();

            if is_binary_extension(path) {
                continue;
            }

            if let Some(scanned) = self.build_scanned_file(relative_base, path) {
                files.push(scanned);
            }
        }

        Ok(files)
    }

    /// Read metadata + contents for `path` and assemble a [`ScannedFile`].
    ///
    /// Returns `None` (after a warn) when the file is too large or any IO step
    /// fails, so the caller can simply skip it.
    fn build_scanned_file(&self, root: &Path, path: &Path) -> Option<ScannedFile> {
        let metadata = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "skipping file: cannot read metadata");
                return None;
            }
        };

        let size = metadata.len();
        if size > MAX_FILE_SIZE {
            tracing::debug!(path = %path.display(), size, "skipping file: exceeds size limit");
            return None;
        }

        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "skipping file: cannot read contents");
                return None;
            }
        };

        let content_hash = blake3::hash(&bytes).to_hex().to_string();
        let relative_path = to_posix_relative(root, path)?;
        let language = Language::from_path(path);

        Some(ScannedFile {
            path: path.to_path_buf(),
            relative_path,
            language,
            size,
            content_hash,
        })
    }
}

/// True if `path`'s extension is a known binary/non-source extension.
fn is_binary_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| BINARY_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Compute `path` relative to `root` as a POSIX-style [`PathBuf`]
/// (forward-slash separators), regardless of host platform.
fn to_posix_relative(root: &Path, path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(root).ok()?;
    let posix = rel
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Some(PathBuf::from(posix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    /// Collect the POSIX relative paths of a scan result into a sorted set for
    /// order-independent assertions.
    fn relative_paths(files: &[ScannedFile]) -> BTreeSet<String> {
        files
            .iter()
            .map(|f| f.relative_path.to_string_lossy().into_owned())
            .collect()
    }

    fn write_file(dir: &Path, rel: &str, contents: &[u8]) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn scan_temp(dir: &TempDir) -> Vec<ScannedFile> {
        let scanner = FileScanner::new(dir.path()).unwrap();
        scanner.scan(dir.path()).unwrap()
    }

    #[test]
    fn scans_nested_directories_recursively() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "main.rs", b"fn main() {}");
        write_file(dir.path(), "src/lib.rs", b"pub fn a() {}");
        write_file(dir.path(), "src/util/helpers.py", b"def f(): pass");

        let files = scan_temp(&dir);
        let paths = relative_paths(&files);

        assert!(paths.contains("main.rs"));
        assert!(paths.contains("src/lib.rs"));
        assert!(paths.contains("src/util/helpers.py"));
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn skips_blacklisted_directories() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "keep.rs", b"fn main() {}");
        write_file(dir.path(), ".git/config", b"[core]");
        write_file(
            dir.path(),
            "node_modules/pkg/index.js",
            b"module.exports={}",
        );
        write_file(dir.path(), "target/debug/build.rs", b"fn main() {}");
        write_file(dir.path(), "dist/bundle.js", b"console.log(1)");
        write_file(dir.path(), "build/out.js", b"console.log(1)");
        write_file(dir.path(), "__pycache__/mod.py", b"x=1");
        write_file(dir.path(), ".venv/lib.py", b"x=1");

        let paths = relative_paths(&scan_temp(&dir));

        assert_eq!(paths.len(), 1);
        assert!(paths.contains("keep.rs"));
    }

    #[test]
    fn filters_binary_extensions() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "code.ts", b"export const x = 1;");
        write_file(dir.path(), "logo.png", b"\x89PNG\r\n");
        write_file(dir.path(), "archive.zip", b"PK\x03\x04");
        write_file(dir.path(), "font.woff2", b"wOF2");
        write_file(dir.path(), "lib.dll", b"MZ");

        let paths = relative_paths(&scan_temp(&dir));

        assert_eq!(paths.len(), 1);
        assert!(paths.contains("code.ts"));
    }

    #[test]
    fn filters_files_larger_than_limit() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "small.rs", b"fn a() {}");
        let big = vec![b'x'; (MAX_FILE_SIZE + 1) as usize];
        write_file(dir.path(), "big.rs", &big);

        let paths = relative_paths(&scan_temp(&dir));

        assert_eq!(paths.len(), 1);
        assert!(paths.contains("small.rs"));
        assert!(!paths.contains("big.rs"));
    }

    #[test]
    fn respects_gitignore() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), ".gitignore", b"ignored.rs\nsecret/\n*.log\n");
        write_file(dir.path(), "kept.rs", b"fn a() {}");
        write_file(dir.path(), "ignored.rs", b"fn b() {}");
        write_file(dir.path(), "secret/data.rs", b"fn c() {}");
        write_file(dir.path(), "app.log", b"log line");

        let paths = relative_paths(&scan_temp(&dir));

        assert!(paths.contains("kept.rs"));
        assert!(!paths.contains("ignored.rs"));
        assert!(!paths.contains("secret/data.rs"));
        assert!(!paths.contains("app.log"));
    }

    #[test]
    fn content_hash_is_stable_for_same_content_and_differs_for_different() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "a.rs", b"identical contents");
        write_file(dir.path(), "b.rs", b"identical contents");
        write_file(dir.path(), "c.rs", b"different contents");

        let files = scan_temp(&dir);
        let hash_of = |name: &str| {
            files
                .iter()
                .find(|f| f.relative_path.to_string_lossy() == name)
                .map(|f| f.content_hash.clone())
                .unwrap()
        };

        assert_eq!(hash_of("a.rs"), hash_of("b.rs"));
        assert_ne!(hash_of("a.rs"), hash_of("c.rs"));
    }

    #[test]
    fn relative_path_is_posix_style() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/deep/nested/mod.rs", b"fn a() {}");

        let files = scan_temp(&dir);
        let file = files
            .iter()
            .find(|f| f.relative_path.to_string_lossy().contains("mod.rs"))
            .unwrap();

        let rel = file.relative_path.to_string_lossy();
        assert_eq!(rel, "src/deep/nested/mod.rs");
        assert!(
            !rel.contains('\\'),
            "relative path must use forward slashes"
        );
    }

    #[test]
    fn detects_language_and_absolute_path() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "main.rs", b"fn main() {}");
        write_file(dir.path(), "notes.txt", b"hello");

        let files = scan_temp(&dir);

        let rust = files
            .iter()
            .find(|f| f.relative_path.to_string_lossy() == "main.rs")
            .unwrap();
        assert_eq!(rust.language, Language::Rust);
        assert!(rust.path.is_absolute());

        let other = files
            .iter()
            .find(|f| f.relative_path.to_string_lossy() == "notes.txt")
            .unwrap();
        assert_eq!(other.language, Language::Other);
    }
}
