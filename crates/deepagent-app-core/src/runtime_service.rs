//! Managed-runtime manager (office-agent Phase 2).
//!
//! Heavy runtimes (speech models, pdfium, pandoc, LibreOffice) are **not**
//! bundled in the installer. They are downloaded on demand into the configured
//! active runtimes directory, not `PATH`, and verified by SHA-256 before
//! install. Legacy runtime directories can remain as read-only lookup roots so
//! moving resources does not break already-installed capabilities. When a
//! runtime is absent the
//! caller falls back to a pure-Rust Tier C path (handled by the consumer).
//!
//! The core (registry / resolution / install-dir selection / checksum verify /
//! archive extract) is pure Rust and unit-tested with a mock downloader. The
//! actual HTTP fetch is an injected [`Downloader`]; a reqwest-backed
//! implementation lives behind the `runtimes` feature.
//!
//! Integrity is **fail-closed**: an artifact without a pinned SHA-256 cannot be
//! installed. This is deliberate: real registry entries must pin a verified
//! checksum before the download path is enabled.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use sha2::{Digest, Sha256};

use crate::dto::{RuntimeProgressDto, RuntimeStatusDto};

/// Target platform of a runtime artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    WindowsX64,
    WindowsArm64,
    MacOsX64,
    MacOsArm64,
    LinuxX64,
    LinuxArm64,
}

impl Platform {
    /// The platform this binary was built for.
    pub fn current() -> Platform {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            Platform::WindowsX64
        }
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        {
            Platform::WindowsArm64
        }
        #[cfg(all(
            target_os = "windows",
            not(any(target_arch = "x86_64", target_arch = "aarch64"))
        ))]
        {
            Platform::WindowsX64
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Platform::MacOsArm64
        }
        #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
        {
            Platform::MacOsX64
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            Platform::LinuxArm64
        }
        #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
        {
            Platform::LinuxX64
        }
        #[cfg(all(
            not(target_os = "windows"),
            not(target_os = "macos"),
            not(target_os = "linux")
        ))]
        {
            Platform::LinuxX64
        }
    }
}

/// How a downloaded artifact is materialized into the install dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// A zip archive extracted into the destination subdir.
    Zip,
    /// A gzip-compressed tar archive extracted into the destination subdir.
    TarGz,
    /// A single raw file (e.g. a `.bin` model) copied as-is.
    Raw,
}

/// One downloadable artifact for a specific platform.
#[derive(Debug, Clone)]
pub struct RuntimeArtifact {
    /// HTTPS download URL (fixed, trusted source).
    pub url: String,
    /// Optional fallback URLs for networks where the primary host is blocked.
    /// These are tried after `url`; downloaded bytes are still verified.
    pub mirror_urls: Vec<String>,
    /// Pinned SHA-256 (lowercase hex). `None` means not installable
    /// (fail-closed).
    pub sha256: Option<String>,
    /// Destination subdir under the install root (e.g. "speech/models").
    pub dest_subdir: String,
    /// For [`ArchiveKind::Raw`], the on-disk file name to write.
    pub file_name: String,
    /// Whether the artifact is a zip to extract or a raw file to copy.
    pub archive: ArchiveKind,
    /// Optional platform-specific probe path relative to `dest_subdir`.
    pub probe: Option<String>,
    /// Optional multi-file payload. When non-empty this artifact is installed
    /// as a directory snapshot instead of a single raw/archive file.
    pub files: Vec<RuntimeFileArtifact>,
    /// Allows files without SHA-256 only for immutable HTTPS snapshots whose
    /// revision is pinned in the URL.
    pub allow_unpinned_files: bool,
}

/// One file inside a multi-file runtime artifact.
#[derive(Debug, Clone)]
pub struct RuntimeFileArtifact {
    /// Relative path under the runtime destination directory.
    pub path: String,
    /// HTTPS download URL.
    pub url: String,
    /// Optional fallback URLs.
    pub mirror_urls: Vec<String>,
    /// Optional SHA-256 for this file. Files with a hash are verified.
    pub sha256: Option<String>,
    /// Optional expected size, used for aggregate progress.
    pub size_bytes: Option<u64>,
}

/// A managed runtime: an id, the capability it provides, and per-platform
/// artifacts. `probe` is a path (relative to `dest_subdir`) whose existence
/// means "installed".
#[derive(Debug, Clone)]
pub struct RuntimeEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capability: String,
    pub size_bytes: u64,
    pub artifacts: HashMap<Platform, RuntimeArtifact>,
    pub probe: String,
}

impl RuntimeEntry {
    fn artifact(&self) -> Option<&RuntimeArtifact> {
        self.artifacts.get(&Platform::current())
    }
}

/// Progress callback: `(downloaded_bytes, total_bytes_when_known)`.
pub type ProgressFn = dyn Fn(u64, Option<u64>) + Send + Sync;

/// Abstracts the HTTP fetch so the core is testable without a network. The
/// real implementation lives behind the `runtimes` feature.
#[async_trait]
pub trait Downloader: Send + Sync {
    /// Download `url` to `dest`, honoring `cancel`, reporting via `progress`.
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        cancel: &AtomicBool,
        progress: &ProgressFn,
    ) -> Result<()>;
}

/// A downloader that always errors: the default when no real downloader is
/// injected (e.g. the kernel workspace without the `runtimes` feature).
pub struct UnavailableDownloader;

#[async_trait]
impl Downloader for UnavailableDownloader {
    async fn download(
        &self,
        _url: &str,
        _dest: &Path,
        _cancel: &AtomicBool,
        _progress: &ProgressFn,
    ) -> Result<()> {
        Err(CoreError::Other(
            "runtime downloading is not enabled in this build".to_string(),
        ))
    }
}

/// Manages download + verified install of optional runtimes.
pub struct RuntimeService {
    registry: Vec<RuntimeEntry>,
    /// The writable target for downloads, installs, and uninstalls.
    active_root: PathBuf,
    /// Read-only lookup roots used for compatibility with older installs.
    lookup_roots: Vec<PathBuf>,
    downloader: Arc<dyn Downloader>,
    /// Per-id cancellation flags for in-flight installs.
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl RuntimeService {
    /// Build with the default registry. The first candidate is the preferred
    /// active root; later candidates are read-only fallback roots.
    pub fn new(candidates: &[PathBuf], downloader: Arc<dyn Downloader>) -> Self {
        Self::with_registry(candidates, downloader, default_registry())
    }

    /// Build with an explicit registry (used by tests).
    pub fn with_registry(
        candidates: &[PathBuf],
        downloader: Arc<dyn Downloader>,
        registry: Vec<RuntimeEntry>,
    ) -> Self {
        Self::with_registry_and_lookup(candidates, &[], downloader, registry)
    }

    /// Build with explicit active-root candidates and read-only lookup roots.
    /// Downloads only target the resolved active root; lookup roots are used to
    /// keep older app-data/exe runtime installs usable until the user migrates.
    pub fn with_lookup_roots(
        active_candidates: &[PathBuf],
        lookup_roots: &[PathBuf],
        downloader: Arc<dyn Downloader>,
    ) -> Self {
        Self::with_registry_and_lookup(
            active_candidates,
            lookup_roots,
            downloader,
            default_registry(),
        )
    }

    /// Build with an explicit registry and split active/read-only roots.
    pub fn with_registry_and_lookup(
        active_candidates: &[PathBuf],
        read_only_roots: &[PathBuf],
        downloader: Arc<dyn Downloader>,
        registry: Vec<RuntimeEntry>,
    ) -> Self {
        let active_root = resolve_active_root(active_candidates);
        let lookup_roots = normalize_lookup_roots(&active_root, active_candidates, read_only_roots);
        Self {
            registry,
            active_root,
            lookup_roots,
            downloader,
            cancels: Mutex::new(HashMap::new()),
        }
    }

    /// The active install root. Downloads and uninstalls only touch this root.
    pub fn install_root(&self) -> &Path {
        &self.active_root
    }

    /// All lookup roots in precedence order. The active root is always first.
    pub fn lookup_roots(&self) -> &[PathBuf] {
        &self.lookup_roots
    }

    /// True iff the runtime `id` is installed (its probe path exists).
    pub fn is_installed(&self, id: &str) -> bool {
        self.installed_path(id).is_some()
    }

    /// Absolute install dir for `id` when installed, else `None`.
    pub fn installed_path(&self, id: &str) -> Option<PathBuf> {
        self.installed_location(id).map(|(path, _source)| path)
    }

    fn installed_location(&self, id: &str) -> Option<(PathBuf, &'static str)> {
        let entry = self.registry.iter().find(|e| e.id == id)?;
        let artifact = entry.artifact()?;
        for root in &self.lookup_roots {
            let dir = root.join(&artifact.dest_subdir);
            let probe = dir.join(artifact.probe.as_deref().unwrap_or(&entry.probe));
            if probe.exists() {
                let source = if same_path(root, &self.active_root) {
                    "active"
                } else {
                    "fallback"
                };
                return Some((dir, source));
            }
        }
        None
    }

    /// Resolve a capability to its installed runtime dir (Tier R), or `None`
    /// when no providing runtime is installed (caller falls back to Tier C).
    pub fn resolve(&self, capability: &str) -> Option<PathBuf> {
        self.registry
            .iter()
            .filter(|e| e.capability == capability)
            .find_map(|e| self.installed_path(&e.id))
    }

    /// List all known runtimes with their status (for the UI catalog).
    pub fn list(&self) -> Vec<RuntimeStatusDto> {
        self.registry.iter().map(|e| self.status_of(e)).collect()
    }

    /// Status of one runtime by id.
    pub fn status(&self, id: &str) -> Option<RuntimeStatusDto> {
        self.registry
            .iter()
            .find(|e| e.id == id)
            .map(|e| self.status_of(e))
    }

    fn status_of(&self, entry: &RuntimeEntry) -> RuntimeStatusDto {
        let artifact = entry.artifact();
        let installed = self.installed_location(&entry.id);
        RuntimeStatusDto {
            id: entry.id.clone(),
            name: entry.name.clone(),
            version: entry.version.clone(),
            capability: entry.capability.clone(),
            size_bytes: entry.size_bytes,
            installed: installed.is_some(),
            available_for_platform: artifact.is_some(),
            checksum_pinned: artifact.map(artifact_integrity_pinned).unwrap_or(false),
            install_path: installed
                .as_ref()
                .map(|(p, _)| p.to_string_lossy().into_owned()),
            install_source: installed.map(|(_, source)| source.to_string()),
        }
    }

    /// Request cancellation of an in-flight install for `id`.
    pub fn cancel(&self, id: &str) {
        if let Ok(map) = self.cancels.lock() {
            if let Some(flag) = map.get(id) {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Uninstall a runtime: remove its destination subdir. Idempotent.
    pub fn uninstall(&self, id: &str) -> Result<bool> {
        let Some(entry) = self.registry.iter().find(|e| e.id == id) else {
            return Ok(false);
        };
        let Some(artifact) = entry.artifact() else {
            return Ok(false);
        };
        let dir = self.active_root.join(&artifact.dest_subdir);
        let probe = dir.join(artifact.probe.as_deref().unwrap_or(&entry.probe));
        if !probe.exists() {
            return Ok(false);
        }
        // For raw single-file artifacts, only remove the file; for zip/tar and
        // multi-file installs, remove the whole destination directory.
        if !artifact.files.is_empty() {
            let _ = std::fs::remove_dir_all(&dir);
        } else {
            match artifact.archive {
                ArchiveKind::Raw => {
                    let f = dir.join(&artifact.file_name);
                    let _ = std::fs::remove_file(f);
                }
                ArchiveKind::Zip | ArchiveKind::TarGz => {
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
        Ok(true)
    }

    /// Download + verify + install the runtime `id`. Reports progress through
    /// `progress` (downloaded/total). Fails closed: no pinned checksum ⇒ error;
    /// checksum mismatch ⇒ the temp file is removed and an error returned.
    pub async fn install(&self, id: &str, progress: Arc<ProgressFn>) -> Result<RuntimeStatusDto> {
        let entry = self
            .registry
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| CoreError::Other(format!("unknown runtime '{id}'")))?
            .clone();
        let artifact = entry
            .artifact()
            .ok_or_else(|| CoreError::Other(format!("'{id}' has no artifact for this platform")))?
            .clone();
        if !artifact.files.is_empty() {
            std::fs::create_dir_all(&self.active_root)
                .map_err(|e| CoreError::Other(format!("create runtimes dir: {e}")))?;
            let cancel = Arc::new(AtomicBool::new(false));
            if let Ok(mut map) = self.cancels.lock() {
                map.insert(id.to_string(), cancel.clone());
            }
            let install_result = self
                .install_multi_file_artifact(id, &artifact, &cancel, progress)
                .await;
            self.clear_cancel(id);
            install_result?;
            return self
                .status(id)
                .ok_or_else(|| CoreError::Other(format!("runtime '{id}' vanished after install")));
        }
        let expected = artifact.sha256.clone().ok_or_else(|| {
            CoreError::Other(format!(
                "'{id}' has no pinned SHA-256 — refusing to install unverified runtime"
            ))
        })?;

        std::fs::create_dir_all(&self.active_root)
            .map_err(|e| CoreError::Other(format!("create runtimes dir: {e}")))?;
        let tmp = self.active_root.join(format!(".dl-{id}.tmp"));

        // Register a cancel flag for this install.
        let cancel = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = self.cancels.lock() {
            map.insert(id.to_string(), cancel.clone());
        }

        // Download. Try the canonical URL first, then configured mirrors.
        let mut urls = Vec::with_capacity(1 + artifact.mirror_urls.len());
        urls.push(artifact.url.clone());
        urls.extend(artifact.mirror_urls.clone());
        let mut last_error = None;
        for url in &urls {
            let dl_progress = progress.clone();
            match self
                .downloader
                .download(url, &tmp, &cancel, &move |d, t| dl_progress(d, t))
                .await
            {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    if cancel.load(Ordering::SeqCst) {
                        self.clear_cancel(id);
                        return Err(e);
                    }
                    last_error = Some(format!("{url}: {e}"));
                }
            }
        }
        if let Some(e) = last_error {
            self.clear_cancel(id);
            return Err(CoreError::Other(format!(
                "download failed for '{id}' after trying {} URL(s): {e}",
                urls.len()
            )));
        }

        // Verify.
        let actual = match sha256_file(&tmp) {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                self.clear_cancel(id);
                return Err(e);
            }
        };
        if actual != expected.to_lowercase() {
            let _ = std::fs::remove_file(&tmp);
            self.clear_cancel(id);
            return Err(CoreError::Other(format!(
                "checksum mismatch for '{id}': expected {expected}, got {actual}"
            )));
        }

        // Install.
        let dest_dir = self.active_root.join(&artifact.dest_subdir);
        let install_result = match artifact.archive {
            ArchiveKind::Zip => extract_zip_into(&tmp, &dest_dir),
            ArchiveKind::TarGz => extract_tar_gz_into(&tmp, &dest_dir),
            ArchiveKind::Raw => copy_raw(&tmp, &dest_dir, &artifact.file_name),
        };
        let _ = std::fs::remove_file(&tmp);
        self.clear_cancel(id);
        install_result?;

        self.status(id)
            .ok_or_else(|| CoreError::Other(format!("runtime '{id}' vanished after install")))
    }

    async fn install_multi_file_artifact(
        &self,
        id: &str,
        artifact: &RuntimeArtifact,
        cancel: &AtomicBool,
        progress: Arc<ProgressFn>,
    ) -> Result<()> {
        if !artifact_integrity_pinned(artifact) {
            return Err(CoreError::Other(format!(
                "'{id}' does not have a pinned runtime snapshot - refusing to install"
            )));
        }
        if !artifact.allow_unpinned_files && artifact.files.iter().any(|file| file.sha256.is_none())
        {
            return Err(CoreError::Other(format!(
                "'{id}' has unpinned files - refusing to install unverified runtime"
            )));
        }

        let dest_dir = self.active_root.join(&artifact.dest_subdir);
        let stage_dir = self.active_root.join(format!(".install-{id}.tmp"));
        let _ = std::fs::remove_dir_all(&stage_dir);
        std::fs::create_dir_all(&stage_dir)
            .map_err(|e| CoreError::Other(format!("create staging dir: {e}")))?;

        let total = aggregate_file_size(&artifact.files);
        let mut completed = 0u64;
        for file in &artifact.files {
            if cancel.load(Ordering::SeqCst) {
                let _ = std::fs::remove_dir_all(&stage_dir);
                return Err(CoreError::Other("download cancelled".to_string()));
            }

            let rel = safe_relative_path(&file.path)?;
            let out = stage_dir.join(rel);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::Other(format!("create model file parent: {e}")))?;
            }

            download_with_mirrors(
                self.downloader.as_ref(),
                file,
                &out,
                cancel,
                progress.clone(),
                completed,
                total,
            )
            .await
            .inspect_err(|_e| {
                let _ = std::fs::remove_dir_all(&stage_dir);
            })?;

            if let Some(expected) = file.sha256.as_ref() {
                let actual = sha256_file(&out).inspect_err(|_e| {
                    let _ = std::fs::remove_dir_all(&stage_dir);
                })?;
                if actual != expected.to_lowercase() {
                    let _ = std::fs::remove_dir_all(&stage_dir);
                    return Err(CoreError::Other(format!(
                        "checksum mismatch for '{id}' file '{}': expected {expected}, got {actual}",
                        file.path
                    )));
                }
            }

            completed = completed.saturating_add(file.size_bytes.unwrap_or_else(|| {
                std::fs::metadata(&out)
                    .map(|meta| meta.len())
                    .unwrap_or_default()
            }));
            progress(completed, total);
        }

        let _ = std::fs::remove_dir_all(&dest_dir);
        if let Some(parent) = dest_dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Other(format!("create runtime parent: {e}")))?;
        }
        std::fs::rename(&stage_dir, &dest_dir)
            .map_err(|e| CoreError::Other(format!("install runtime directory: {e}")))?;
        Ok(())
    }

    fn clear_cancel(&self, id: &str) {
        if let Ok(mut map) = self.cancels.lock() {
            map.remove(id);
        }
    }
}

/// Pick the writable active root. The first candidate is preferred; later
/// candidates are fallbacks only when the preferred target cannot be written.
fn resolve_active_root(candidates: &[PathBuf]) -> PathBuf {
    for c in candidates {
        if std::fs::create_dir_all(c).is_ok() && is_writable_dir(c) {
            return c.clone();
        }
    }
    candidates
        .first()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("deepagent-runtimes"))
}

fn normalize_lookup_roots(
    active_root: &Path,
    active_candidates: &[PathBuf],
    read_only_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut roots = vec![active_root.to_path_buf()];
    for candidate in active_candidates.iter().chain(read_only_roots.iter()) {
        if !same_path(active_root, candidate)
            && !roots.iter().any(|root| same_path(root, candidate))
        {
            roots.push(candidate.clone());
        }
    }
    roots
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn artifact_integrity_pinned(artifact: &RuntimeArtifact) -> bool {
    if artifact.files.is_empty() {
        artifact.sha256.is_some()
    } else {
        artifact.files.iter().all(|file| file.sha256.is_some())
            || (artifact.allow_unpinned_files
                && artifact.files.iter().all(file_url_revision_pinned))
    }
}

fn file_url_revision_pinned(file: &RuntimeFileArtifact) -> bool {
    std::iter::once(&file.url)
        .chain(file.mirror_urls.iter())
        .all(|url| url.starts_with("https://") && !url.contains("/resolve/main/"))
}

fn aggregate_file_size(files: &[RuntimeFileArtifact]) -> Option<u64> {
    files
        .iter()
        .try_fold(0u64, |sum, file| file.size_bytes.map(|size| sum + size))
}

fn safe_relative_path(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(CoreError::Other(format!(
            "unsafe runtime file path '{path}'"
        )));
    }
    Ok(p.to_path_buf())
}

async fn download_with_mirrors(
    downloader: &dyn Downloader,
    file: &RuntimeFileArtifact,
    dest: &Path,
    cancel: &AtomicBool,
    progress: Arc<ProgressFn>,
    completed: u64,
    total: Option<u64>,
) -> Result<()> {
    let mut urls = Vec::with_capacity(1 + file.mirror_urls.len());
    urls.push(file.url.clone());
    urls.extend(file.mirror_urls.clone());
    let mut last_error = None;
    for url in &urls {
        let dl_progress = progress.clone();
        let file_size = file.size_bytes;
        let report_total = total;
        match downloader
            .download(url, dest, cancel, &move |downloaded, downloaded_total| {
                let current_total = report_total.or_else(|| {
                    downloaded_total.map(|inner_total| completed.saturating_add(inner_total))
                });
                let current_downloaded = completed
                    .saturating_add(file_size.map_or(downloaded, |size| downloaded.min(size)));
                dl_progress(current_downloaded, current_total);
            })
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(dest);
                if cancel.load(Ordering::SeqCst) {
                    return Err(e);
                }
                last_error = Some(format!("{url}: {e}"));
            }
        }
    }
    Err(CoreError::Other(format!(
        "download failed for file '{}' after trying {} URL(s): {}",
        file.path,
        urls.len(),
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )))
}

/// Probe writability by creating and removing a marker file.
fn is_writable_dir(dir: &Path) -> bool {
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Stream a file through SHA-256, returning the lowercase hex digest.
fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| CoreError::Other(format!("open for hash: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| CoreError::Other(format!("read for hash: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// Lowercase hex of a byte slice.
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Extract a zip file into `dest_dir` (created if missing).
fn extract_zip_into(zip_path: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| CoreError::Other(format!("create dest dir: {e}")))?;
    let file =
        std::fs::File::open(zip_path).map_err(|e| CoreError::Other(format!("open zip: {e}")))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| CoreError::Other(format!("read zip: {e}")))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| CoreError::Other(format!("zip entry {i}: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            continue; // skip unsafe paths (zip-slip guard)
        };
        let out = dest_dir.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| CoreError::Other(format!("create dir: {e}")))?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::Other(format!("create parent: {e}")))?;
            }
            let mut outfile = std::fs::File::create(&out)
                .map_err(|e| CoreError::Other(format!("create file: {e}")))?;
            std::io::copy(&mut entry, &mut outfile)
                .map_err(|e| CoreError::Other(format!("write file: {e}")))?;
        }
    }
    Ok(())
}

/// Copy a raw downloaded file into `dest_dir` under `file_name`.
fn copy_raw(src: &Path, dest_dir: &Path, file_name: &str) -> Result<()> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| CoreError::Other(format!("create dest dir: {e}")))?;
    std::fs::copy(src, dest_dir.join(file_name))
        .map_err(|e| CoreError::Other(format!("copy runtime file: {e}")))?;
    Ok(())
}

/// Extract a gzip-compressed tar archive into `dest_dir` (created if missing).
/// The `tar` crate guards against path traversal via `Entry::unpack_in`.
fn extract_tar_gz_into(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| CoreError::Other(format!("create dest dir: {e}")))?;
    let file = std::fs::File::open(archive_path)
        .map_err(|e| CoreError::Other(format!("open tar.gz: {e}")))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(dest_dir)
        .map_err(|e| CoreError::Other(format!("extract tar.gz: {e}")))?;
    Ok(())
}

/// The default runtime registry. Entries are listed in the UI catalog so users
/// can see what's available; **install is blocked until a SHA-256 is pinned**
/// (artifacts ship `sha256: None` here as a fail-closed placeholder — the
/// verified hash is pinned when the real download path is enabled).
pub fn default_registry() -> Vec<RuntimeEntry> {
    // Data models can be shared across OSes; pass `Some(sha256)` only after the
    // upstream artifact hash has been verified and pinned.
    fn all_platforms(
        url: &str,
        mirror_urls: &[&str],
        sha256: Option<&str>,
        dest: &str,
        file: &str,
        archive: ArchiveKind,
    ) -> HashMap<Platform, RuntimeArtifact> {
        // Speech models are platform-independent data; office binaries differ
        // per platform. For now we register the common (data) artifacts for all
        // platforms and leave OS-specific binaries to be filled per platform.
        let mut m = HashMap::new();
        for p in [
            Platform::WindowsX64,
            Platform::WindowsArm64,
            Platform::MacOsX64,
            Platform::MacOsArm64,
            Platform::LinuxX64,
            Platform::LinuxArm64,
        ] {
            m.insert(
                p,
                RuntimeArtifact {
                    url: url.to_string(),
                    mirror_urls: mirror_urls.iter().map(|u| (*u).to_string()).collect(),
                    sha256: sha256.map(str::to_string),
                    dest_subdir: dest.to_string(),
                    file_name: file.to_string(),
                    archive,
                    probe: None,
                    files: Vec::new(),
                    allow_unpinned_files: false,
                },
            );
        }
        m
    }

    fn all_platforms_files(
        dest: &str,
        files: Vec<RuntimeFileArtifact>,
        allow_unpinned_files: bool,
    ) -> HashMap<Platform, RuntimeArtifact> {
        let mut m = HashMap::new();
        for p in [
            Platform::WindowsX64,
            Platform::WindowsArm64,
            Platform::MacOsX64,
            Platform::MacOsArm64,
            Platform::LinuxX64,
            Platform::LinuxArm64,
        ] {
            m.insert(
                p,
                RuntimeArtifact {
                    url: String::new(),
                    mirror_urls: Vec::new(),
                    sha256: None,
                    dest_subdir: dest.to_string(),
                    file_name: String::new(),
                    archive: ArchiveKind::Raw,
                    probe: None,
                    files: files.clone(),
                    allow_unpinned_files,
                },
            );
        }
        m
    }

    fn hf_file(
        repo: &str,
        revision: &str,
        path: &str,
        size_bytes: Option<u64>,
        sha256: Option<&str>,
    ) -> RuntimeFileArtifact {
        RuntimeFileArtifact {
            path: path.to_string(),
            url: format!("https://huggingface.co/{repo}/resolve/{revision}/{path}"),
            mirror_urls: vec![format!(
                "https://hf-mirror.com/{repo}/resolve/{revision}/{path}"
            )],
            sha256: sha256.map(str::to_string),
            size_bytes,
        }
    }

    let mut whisper_cli_artifacts = per_platform_pinned([
        (
            Platform::WindowsX64,
            "https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-x64.zip",
            &[
                "https://gh.llkk.cc/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-x64.zip",
                "https://gh-proxy.com/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-x64.zip",
            ],
            "7d8be46ecd31828e1eb7a2ecdd0d6b314feafd82163038ab6092594b0a063539",
            "speech/whisper-cli",
            "whisper-bin-x64.zip",
            ArchiveKind::Zip,
            "Release/whisper-cli.exe",
        ),
        (
            Platform::LinuxX64,
            "https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-ubuntu-x64.tar.gz",
            &[
                "https://gh.llkk.cc/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-ubuntu-x64.tar.gz",
                "https://gh-proxy.com/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-ubuntu-x64.tar.gz",
            ],
            "f3bf3b4369a99b54665b0f19b88483b30de27f25963b0414235dea03198515c5",
            "speech/whisper-cli",
            "whisper-bin-ubuntu-x64.tar.gz",
            ArchiveKind::TarGz,
            "whisper-bin-ubuntu-x64/whisper-cli",
        ),
        (
            Platform::LinuxArm64,
            "https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-ubuntu-arm64.tar.gz",
            &[
                "https://gh.llkk.cc/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-ubuntu-arm64.tar.gz",
                "https://gh-proxy.com/https://github.com/ggml-org/whisper.cpp/releases/download/v1.9.1/whisper-bin-ubuntu-arm64.tar.gz",
            ],
            "e0b66cd551ff6f2a28fabe3c6e89691eea037bb76833493abb9a71ca788994b3",
            "speech/whisper-cli",
            "whisper-bin-ubuntu-arm64.tar.gz",
            ArchiveKind::TarGz,
            "whisper-bin-ubuntu-arm64/whisper-cli",
        ),
    ]);
    insert_platform_artifact(
        &mut whisper_cli_artifacts,
        Platform::MacOsX64,
        "https://github.com/eighteendreamer/DeepAgent-Studio/releases/download/runtime-whisper-cli-v1.9.1/deepagent-whisper-cli-macos-x64.tar.gz",
        &[
            "https://gh.llkk.cc/https://github.com/eighteendreamer/DeepAgent-Studio/releases/download/runtime-whisper-cli-v1.9.1/deepagent-whisper-cli-macos-x64.tar.gz",
            "https://gh-proxy.com/https://github.com/eighteendreamer/DeepAgent-Studio/releases/download/runtime-whisper-cli-v1.9.1/deepagent-whisper-cli-macos-x64.tar.gz",
        ],
        option_env!("DEEPAGENT_WHISPER_CLI_MACOS_X64_SHA256"),
        "speech/whisper-cli",
        "deepagent-whisper-cli-macos-x64.tar.gz",
        ArchiveKind::TarGz,
        "deepagent-whisper-cli-macos-x64/whisper-cli",
    );
    insert_platform_artifact(
        &mut whisper_cli_artifacts,
        Platform::MacOsArm64,
        "https://github.com/eighteendreamer/DeepAgent-Studio/releases/download/runtime-whisper-cli-v1.9.1/deepagent-whisper-cli-macos-arm64.tar.gz",
        &[
            "https://gh.llkk.cc/https://github.com/eighteendreamer/DeepAgent-Studio/releases/download/runtime-whisper-cli-v1.9.1/deepagent-whisper-cli-macos-arm64.tar.gz",
            "https://gh-proxy.com/https://github.com/eighteendreamer/DeepAgent-Studio/releases/download/runtime-whisper-cli-v1.9.1/deepagent-whisper-cli-macos-arm64.tar.gz",
        ],
        option_env!("DEEPAGENT_WHISPER_CLI_MACOS_ARM64_SHA256"),
        "speech/whisper-cli",
        "deepagent-whisper-cli-macos-arm64.tar.gz",
        ArchiveKind::TarGz,
        "deepagent-whisper-cli-macos-arm64/whisper-cli",
    );

    vec![
        RuntimeEntry {
            id: "whisper-base".to_string(),
            name: "Whisper base model".to_string(),
            version: "ggml-base".to_string(),
            capability: "speech-model".to_string(),
            size_bytes: 142 * 1024 * 1024,
            artifacts: all_platforms(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
                &["https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"],
                Some("60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"),
                "speech/models",
                "ggml-base.bin",
                ArchiveKind::Raw,
            ),
            probe: "ggml-base.bin".to_string(),
        },
        RuntimeEntry {
            id: "whisper-small".to_string(),
            name: "Whisper small model".to_string(),
            version: "ggml-small".to_string(),
            capability: "speech-model".to_string(),
            size_bytes: 466 * 1024 * 1024,
            artifacts: all_platforms(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
                &["https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"],
                Some("1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"),
                "speech/models",
                "ggml-small.bin",
                ArchiveKind::Raw,
            ),
            probe: "ggml-small.bin".to_string(),
        },
        // pandoc — high-fidelity Markdown↔docx conversion (Tier R doc-convert).
        RuntimeEntry {
            id: "whisper-cli".to_string(),
            name: "Whisper.cpp CLI".to_string(),
            version: "v1.9.1".to_string(),
            capability: "speech-engine".to_string(),
            size_bytes: 8 * 1024 * 1024,
            artifacts: whisper_cli_artifacts,
            probe: "whisper-cli".to_string(),
        },
        RuntimeEntry {
            id: "pandoc".to_string(),
            name: "Pandoc".to_string(),
            version: "3.1.11".to_string(),
            capability: "doc-convert".to_string(),
            size_bytes: 180 * 1024 * 1024,
            artifacts: per_platform([
                (
                    Platform::WindowsX64,
                    "https://github.com/jgm/pandoc/releases/download/3.1.11/pandoc-3.1.11-windows-x86_64.zip",
                    "office/pandoc",
                    "pandoc.zip",
                    ArchiveKind::Zip,
                ),
                (
                    Platform::MacOsX64,
                    "https://github.com/jgm/pandoc/releases/download/3.1.11/pandoc-3.1.11-x86_64-macOS.zip",
                    "office/pandoc",
                    "pandoc.zip",
                    ArchiveKind::Zip,
                ),
                (
                    Platform::LinuxX64,
                    "https://github.com/jgm/pandoc/releases/download/3.1.11/pandoc-3.1.11-linux-amd64.tar.gz",
                    "office/pandoc",
                    "pandoc.tar.gz",
                    ArchiveKind::TarGz,
                ),
            ]),
            // Pandoc archives extract to a versioned `pandoc-<ver>/bin/pandoc`;
            // OfficeService probes for the executable recursively at use time.
            probe: ".".to_string(),
        },
        // pdfium — PDF page rasterization (Tier R pdf-render).
        RuntimeEntry {
            id: "pdfium".to_string(),
            name: "PDFium".to_string(),
            version: "chromium/6666".to_string(),
            capability: "pdf-render".to_string(),
            size_bytes: 12 * 1024 * 1024,
            artifacts: per_platform([
                (
                    Platform::WindowsX64,
                    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F6666/pdfium-win-x64.tgz",
                    "office/pdfium",
                    "pdfium.tgz",
                    ArchiveKind::TarGz,
                ),
                (
                    Platform::MacOsX64,
                    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F6666/pdfium-mac-x64.tgz",
                    "office/pdfium",
                    "pdfium.tgz",
                    ArchiveKind::TarGz,
                ),
                (
                    Platform::LinuxX64,
                    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F6666/pdfium-linux-x64.tgz",
                    "office/pdfium",
                    "pdfium.tgz",
                    ArchiveKind::TarGz,
                ),
            ]),
            probe: "lib".to_string(),
        },
        // Florence-2-base-ft — lightweight local image-to-text extractor for
        // the first system-vision path. It needs a multi-file model manifest;
        // this placeholder keeps the resource visible but fail-closed until
        // pinned checksums and manifest install are added.
        RuntimeEntry {
            id: "vision-florence-2-base-ft".to_string(),
            name: "Florence-2-base-ft".to_string(),
            version: "microsoft/Florence-2-base-ft".to_string(),
            capability: "vision-image-to-text".to_string(),
            size_bytes: 468 * 1024 * 1024,
            artifacts: all_platforms_files(
                "vision/florence-2-base-ft",
                vec![
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "config.json", Some(2_430), None),
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "configuration_florence2.py", Some(15_100), None),
                    hf_file(
                        "microsoft/Florence-2-base-ft",
                        "f6c1a25",
                        "model.safetensors",
                        Some(463_221_266),
                        None,
                    ),
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "modeling_florence2.py", Some(127_000), None),
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "preprocessor_config.json", Some(806), None),
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "processing_florence2.py", Some(46_400), None),
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "tokenizer.json", Some(1_360_000), None),
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "tokenizer_config.json", Some(34), None),
                    hf_file("microsoft/Florence-2-base-ft", "f6c1a25", "vocab.json", Some(1_100_000), None),
                ],
                true,
            ),
            probe: "model.safetensors".to_string(),
        },
        // LibreOffice — legacy formats (.doc/.xls/.ppt) + high-fidelity PDF
        // export (Tier R office-suite). Distribution differs per platform and
        // is large; URLs/hashes are pinned at enablement (fail-closed here).
        RuntimeEntry {
            id: "libreoffice".to_string(),
            name: "LibreOffice (portable)".to_string(),
            version: "24.8".to_string(),
            capability: "office-suite".to_string(),
            size_bytes: 380 * 1024 * 1024,
            artifacts: per_platform([
                (
                    Platform::WindowsX64,
                    "https://download.documentfoundation.org/libreoffice/portable/24.8/LibreOfficePortable_24.8.zip",
                    "office/libreoffice",
                    "libreoffice.zip",
                    ArchiveKind::Zip,
                ),
                (
                    Platform::LinuxX64,
                    "https://download.documentfoundation.org/libreoffice/stable/24.8.0/deb/x86_64/LibreOffice_24.8.0_Linux_x86-64_deb.tar.gz",
                    "office/libreoffice",
                    "libreoffice.tar.gz",
                    ArchiveKind::TarGz,
                ),
            ]),
            probe: ".".to_string(),
        },
    ]
}

/// Build a per-platform artifact map from `(platform, url, dest, file, archive)`
/// tuples. Used for OS-specific binary runtimes (pandoc / pdfium).
fn per_platform<const N: usize>(
    entries: [(Platform, &str, &str, &str, ArchiveKind); N],
) -> HashMap<Platform, RuntimeArtifact> {
    let mut m = HashMap::new();
    for (platform, url, dest, file, archive) in entries {
        m.insert(
            platform,
            RuntimeArtifact {
                url: url.to_string(),
                mirror_urls: Vec::new(),
                sha256: None, // TODO: pin verified checksum before enabling install
                dest_subdir: dest.to_string(),
                file_name: file.to_string(),
                archive,
                probe: None,
                files: Vec::new(),
                allow_unpinned_files: false,
            },
        );
    }
    m
}

type PinnedPlatformArtifact<'a> = (
    Platform,
    &'a str,
    &'a [&'a str],
    &'a str,
    &'a str,
    &'a str,
    ArchiveKind,
    &'a str,
);

fn per_platform_pinned<const N: usize>(
    entries: [PinnedPlatformArtifact<'_>; N],
) -> HashMap<Platform, RuntimeArtifact> {
    let mut m = HashMap::new();
    for (platform, url, mirrors, sha256, dest, file, archive, probe) in entries {
        insert_platform_artifact(
            &mut m,
            platform,
            url,
            mirrors,
            Some(sha256),
            dest,
            file,
            archive,
            probe,
        );
    }
    m
}

#[allow(clippy::too_many_arguments)]
fn insert_platform_artifact(
    artifacts: &mut HashMap<Platform, RuntimeArtifact>,
    platform: Platform,
    url: &str,
    mirrors: &[&str],
    sha256: Option<&str>,
    dest: &str,
    file: &str,
    archive: ArchiveKind,
    probe: &str,
) {
    artifacts.insert(
        platform,
        RuntimeArtifact {
            url: url.to_string(),
            mirror_urls: mirrors.iter().map(|u| (*u).to_string()).collect(),
            sha256: sha256.map(str::to_string),
            dest_subdir: dest.to_string(),
            file_name: file.to_string(),
            archive,
            probe: Some(probe.to_string()),
            files: Vec::new(),
            allow_unpinned_files: false,
        },
    );
}

// ---- reqwest-backed downloader (behind the `runtimes` feature) ------------

/// Real HTTP downloader. Streams the body to disk in chunks, honoring
/// cancellation and reporting progress. Only compiled with `--features runtimes`.
#[cfg(feature = "runtimes")]
pub struct ReqwestDownloader {
    client: reqwest::Client,
}

#[cfg(feature = "runtimes")]
impl Default for ReqwestDownloader {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[cfg(feature = "runtimes")]
#[async_trait]
impl Downloader for ReqwestDownloader {
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        cancel: &AtomicBool,
        progress: &ProgressFn,
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        if !url.starts_with("https://") {
            return Err(CoreError::Other(format!("refusing non-HTTPS url: {url}")));
        }
        let mut resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::Other(format!("request failed: {e}")))?
            .error_for_status()
            .map_err(|e| CoreError::Other(format!("download failed: {e}")))?;
        let total = resp.content_length();
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| CoreError::Other(format!("create temp file: {e}")))?;
        let mut downloaded = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                let _ = file.shutdown().await;
                return Err(CoreError::Other("download cancelled".to_string()));
            }
            match resp
                .chunk()
                .await
                .map_err(|e| CoreError::Other(format!("download stream error: {e}")))?
            {
                Some(chunk) => {
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| CoreError::Other(format!("write temp file: {e}")))?;
                    downloaded += chunk.len() as u64;
                    progress(downloaded, total);
                }
                None => break,
            }
        }
        file.flush()
            .await
            .map_err(|e| CoreError::Other(format!("flush temp file: {e}")))?;
        Ok(())
    }
}

/// Build a [`RuntimeProgressDto`] (convenience for the command layer).
pub fn progress_dto(
    id: &str,
    downloaded: u64,
    total: Option<u64>,
    phase: &str,
) -> RuntimeProgressDto {
    RuntimeProgressDto {
        id: id.to_string(),
        downloaded,
        total,
        phase: phase.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A downloader that writes fixed bytes (no network).
    struct BytesDownloader {
        bytes: Vec<u8>,
    }

    #[async_trait]
    impl Downloader for BytesDownloader {
        async fn download(
            &self,
            _url: &str,
            dest: &Path,
            _cancel: &AtomicBool,
            progress: &ProgressFn,
        ) -> Result<()> {
            std::fs::write(dest, &self.bytes).unwrap();
            progress(self.bytes.len() as u64, Some(self.bytes.len() as u64));
            Ok(())
        }
    }

    struct MirrorFallbackDownloader {
        bytes: Vec<u8>,
        seen_urls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Downloader for MirrorFallbackDownloader {
        async fn download(
            &self,
            url: &str,
            dest: &Path,
            _cancel: &AtomicBool,
            progress: &ProgressFn,
        ) -> Result<()> {
            let mut seen = self.seen_urls.lock().unwrap();
            seen.push(url.to_string());
            if seen.len() == 1 {
                return Err(CoreError::Other("primary blocked".to_string()));
            }
            drop(seen);
            std::fs::write(dest, &self.bytes).unwrap();
            progress(self.bytes.len() as u64, Some(self.bytes.len() as u64));
            Ok(())
        }
    }

    fn sha256_of(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        hex_lower(&h.finalize())
    }

    fn raw_entry(sha: Option<String>) -> RuntimeEntry {
        let mut artifacts = HashMap::new();
        artifacts.insert(
            Platform::current(),
            RuntimeArtifact {
                url: "https://example.com/model.bin".to_string(),
                mirror_urls: Vec::new(),
                sha256: sha,
                dest_subdir: "speech/models".to_string(),
                file_name: "model.bin".to_string(),
                archive: ArchiveKind::Raw,
                probe: None,
                files: Vec::new(),
                allow_unpinned_files: false,
            },
        );
        RuntimeEntry {
            id: "test-model".to_string(),
            name: "Test model".to_string(),
            version: "v1".to_string(),
            capability: "speech-model".to_string(),
            size_bytes: 3,
            artifacts,
            probe: "model.bin".to_string(),
        }
    }

    fn multi_file_entry() -> RuntimeEntry {
        let files = vec![
            RuntimeFileArtifact {
                path: "config.json".to_string(),
                url: "https://example.com/resolve/abc123/config.json".to_string(),
                mirror_urls: Vec::new(),
                sha256: None,
                size_bytes: Some(3),
            },
            RuntimeFileArtifact {
                path: "nested/tokenizer.json".to_string(),
                url: "https://example.com/resolve/abc123/nested/tokenizer.json".to_string(),
                mirror_urls: Vec::new(),
                sha256: None,
                size_bytes: Some(3),
            },
        ];
        let mut artifacts = HashMap::new();
        artifacts.insert(
            Platform::current(),
            RuntimeArtifact {
                url: String::new(),
                mirror_urls: Vec::new(),
                sha256: None,
                dest_subdir: "vision/model".to_string(),
                file_name: String::new(),
                archive: ArchiveKind::Raw,
                probe: None,
                files,
                allow_unpinned_files: true,
            },
        );
        RuntimeEntry {
            id: "vision-test".to_string(),
            name: "Vision Test".to_string(),
            version: "v1".to_string(),
            capability: "vision-image-to-text".to_string(),
            size_bytes: 6,
            artifacts,
            probe: "config.json".to_string(),
        }
    }

    fn noop_progress() -> Arc<ProgressFn> {
        Arc::new(|_d, _t| {})
    }

    #[tokio::test]
    async fn install_verifies_and_places_raw_file() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"abc".to_vec();
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(BytesDownloader {
                bytes: bytes.clone(),
            }),
            vec![raw_entry(Some(sha256_of(&bytes)))],
        );
        assert!(!svc.is_installed("test-model"));
        let status = svc.install("test-model", noop_progress()).await.unwrap();
        assert!(status.installed);
        assert!(svc.is_installed("test-model"));
        assert!(svc.resolve("speech-model").is_some());
        // The file landed under <root>/speech/models/model.bin.
        assert!(dir.path().join("speech/models/model.bin").exists());
    }

    #[tokio::test]
    async fn install_falls_back_to_mirror_url() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"abc".to_vec();
        let mut entry = raw_entry(Some(sha256_of(&bytes)));
        let artifact = entry.artifacts.get_mut(&Platform::current()).unwrap();
        artifact.mirror_urls = vec!["https://mirror.example.com/model.bin".to_string()];
        let downloader = Arc::new(MirrorFallbackDownloader {
            bytes: bytes.clone(),
            seen_urls: Mutex::new(Vec::new()),
        });
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            downloader.clone(),
            vec![entry],
        );
        let status = svc.install("test-model", noop_progress()).await.unwrap();
        assert!(status.installed);
        let seen = downloader.seen_urls.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            [
                "https://example.com/model.bin".to_string(),
                "https://mirror.example.com/model.bin".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn install_places_multi_file_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(BytesDownloader {
                bytes: b"abc".to_vec(),
            }),
            vec![multi_file_entry()],
        );
        let status = svc.install("vision-test", noop_progress()).await.unwrap();
        assert!(status.installed);
        assert!(status.checksum_pinned);
        assert!(dir.path().join("vision/model/config.json").exists());
        assert!(dir
            .path()
            .join("vision/model/nested/tokenizer.json")
            .exists());
    }

    #[tokio::test]
    async fn install_rejects_checksum_mismatch_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(BytesDownloader {
                bytes: b"abc".to_vec(),
            }),
            vec![raw_entry(Some("deadbeef".to_string()))],
        );
        let err = svc
            .install("test-model", noop_progress())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
        assert!(!svc.is_installed("test-model"));
        // Temp file cleaned up.
        assert!(!dir.path().join(".dl-test-model.tmp").exists());
    }

    #[tokio::test]
    async fn install_blocked_without_pinned_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(BytesDownloader {
                bytes: b"abc".to_vec(),
            }),
            vec![raw_entry(None)],
        );
        let err = svc
            .install("test-model", noop_progress())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no pinned SHA-256"));
    }

    #[tokio::test]
    async fn install_extracts_zip() {
        // Build a tiny zip in memory.
        let dir = tempfile::tempdir().unwrap();
        let zip_bytes = {
            let mut buf = Vec::new();
            {
                let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
                let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
                w.start_file("bin/tool.txt", opts).unwrap();
                w.write_all(b"hello").unwrap();
                w.finish().unwrap();
            }
            buf
        };
        let mut artifacts = HashMap::new();
        artifacts.insert(
            Platform::current(),
            RuntimeArtifact {
                url: "https://example.com/tool.zip".to_string(),
                mirror_urls: Vec::new(),
                sha256: Some(sha256_of(&zip_bytes)),
                dest_subdir: "tool".to_string(),
                file_name: "tool.zip".to_string(),
                archive: ArchiveKind::Zip,
                probe: None,
                files: Vec::new(),
                allow_unpinned_files: false,
            },
        );
        let entry = RuntimeEntry {
            id: "tool".to_string(),
            name: "Tool".to_string(),
            version: "v1".to_string(),
            capability: "doc-convert".to_string(),
            size_bytes: zip_bytes.len() as u64,
            artifacts,
            probe: "bin/tool.txt".to_string(),
        };
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(BytesDownloader { bytes: zip_bytes }),
            vec![entry],
        );
        let status = svc.install("tool", noop_progress()).await.unwrap();
        assert!(status.installed);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tool/bin/tool.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn install_extracts_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        // Build a .tar.gz containing bin/tool.txt = "hello".
        let mut targz: Vec<u8> = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut targz, flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "bin/tool.txt", &data[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let mut artifacts = HashMap::new();
        artifacts.insert(
            Platform::current(),
            RuntimeArtifact {
                url: "https://example.com/tool.tgz".to_string(),
                mirror_urls: Vec::new(),
                sha256: Some(sha256_of(&targz)),
                dest_subdir: "tool".to_string(),
                file_name: "tool.tgz".to_string(),
                archive: ArchiveKind::TarGz,
                probe: None,
                files: Vec::new(),
                allow_unpinned_files: false,
            },
        );
        let entry = RuntimeEntry {
            id: "tgztool".to_string(),
            name: "TgzTool".to_string(),
            version: "v1".to_string(),
            capability: "doc-convert".to_string(),
            size_bytes: targz.len() as u64,
            artifacts,
            probe: "bin/tool.txt".to_string(),
        };
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(BytesDownloader { bytes: targz }),
            vec![entry],
        );
        let status = svc.install("tgztool", noop_progress()).await.unwrap();
        assert!(status.installed);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tool/bin/tool.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn default_registry_lists_tier_r_runtimes() {
        let dir = tempfile::tempdir().unwrap();
        let svc = RuntimeService::new(&[dir.path().to_path_buf()], Arc::new(UnavailableDownloader));
        let ids: Vec<String> = svc.list().into_iter().map(|s| s.id).collect();
        assert!(ids.contains(&"pandoc".to_string()));
        assert!(ids.contains(&"pdfium".to_string()));
        assert!(ids.contains(&"vision-florence-2-base-ft".to_string()));
        let whisper_base = svc.status("whisper-base").unwrap();
        let whisper_small = svc.status("whisper-small").unwrap();
        let whisper_cli = svc.status("whisper-cli").unwrap();
        assert!(whisper_base.checksum_pinned);
        assert!(whisper_small.checksum_pinned);
        assert_eq!(whisper_cli.capability, "speech-engine");
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            assert!(whisper_cli.available_for_platform);
            assert!(whisper_cli.checksum_pinned);
        }
        #[cfg(target_os = "macos")]
        {
            assert!(whisper_cli.available_for_platform);
            #[cfg(target_arch = "aarch64")]
            if option_env!("DEEPAGENT_WHISPER_CLI_MACOS_ARM64_SHA256").is_some() {
                assert!(whisper_cli.checksum_pinned);
            } else {
                assert!(!whisper_cli.checksum_pinned);
            }
            #[cfg(not(target_arch = "aarch64"))]
            if option_env!("DEEPAGENT_WHISPER_CLI_MACOS_X64_SHA256").is_some() {
                assert!(whisper_cli.checksum_pinned);
            } else {
                assert!(!whisper_cli.checksum_pinned);
            }
        }
        // Tier R binaries are fail-closed until a checksum is pinned.
        let pandoc = svc.status("pandoc").unwrap();
        assert!(!pandoc.checksum_pinned);
    }

    #[test]
    fn install_root_falls_back_to_writable_candidate() {
        let good = tempfile::tempdir().unwrap();
        // First candidate is a file path (cannot be a dir) → falls through.
        let bad = good.path().join("not-a-dir-marker");
        std::fs::write(&bad, b"x").unwrap();
        let svc = RuntimeService::with_registry(
            &[bad.clone(), good.path().to_path_buf()],
            Arc::new(UnavailableDownloader),
            vec![],
        );
        assert_eq!(svc.install_root(), good.path());
    }

    #[test]
    fn lookup_roots_keep_legacy_runtime_available() {
        let active = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let svc = RuntimeService::with_registry_and_lookup(
            &[active.path().to_path_buf()],
            &[legacy.path().to_path_buf()],
            Arc::new(UnavailableDownloader),
            vec![raw_entry(Some("x".to_string()))],
        );
        let legacy_models = legacy.path().join("speech/models");
        std::fs::create_dir_all(&legacy_models).unwrap();
        std::fs::write(legacy_models.join("model.bin"), b"data").unwrap();

        assert!(svc.is_installed("test-model"));
        assert_eq!(
            svc.resolve("speech-model").unwrap(),
            legacy.path().join("speech/models")
        );
        assert_eq!(svc.install_root(), active.path());
    }

    #[test]
    fn uninstall_removes_installed_raw_file() {
        let dir = tempfile::tempdir().unwrap();
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(UnavailableDownloader),
            vec![raw_entry(Some("x".to_string()))],
        );
        // Place the file manually.
        let models = dir.path().join("speech/models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("model.bin"), b"data").unwrap();
        assert!(svc.is_installed("test-model"));
        assert!(svc.uninstall("test-model").unwrap());
        assert!(!svc.is_installed("test-model"));
    }

    #[test]
    fn list_reports_pinned_and_platform_flags() {
        let dir = tempfile::tempdir().unwrap();
        let svc = RuntimeService::with_registry(
            &[dir.path().to_path_buf()],
            Arc::new(UnavailableDownloader),
            vec![raw_entry(None)],
        );
        let list = svc.list();
        assert_eq!(list.len(), 1);
        assert!(!list[0].checksum_pinned);
        assert!(list[0].available_for_platform);
        assert!(!list[0].installed);
    }
}
