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

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
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
    /// An executable installer (e.g. Inno Setup `.exe`) that is run silently
    /// with `/DIR="<dest>" /VERYSILENT` to install into the managed runtime
    /// directory. The installer itself verifies integrity, so SHA-256 is
    /// optional for this type.
    Installer,
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
    /// Filesystem paths to probe for system-installed binaries. Checked after
    /// the managed runtime directory so software installed via package manager
    /// or manually is still detected as "installed".
    pub system_probe_paths: Vec<String>,
}

/// Executable SDKs shared by terminals, plugins, hooks and built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeKind {
    Node,
    Python,
    Java,
}

impl RuntimeKind {
    fn command(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Python => "python",
            Self::Java => "java",
        }
    }

    fn managed_id(self) -> &'static str {
        match self {
            Self::Node => "node-22",
            Self::Python => "python-3.11",
            Self::Java => "jdk-17",
        }
    }

    fn env_key(self) -> &'static str {
        match self {
            Self::Node => "DEEPAGENT_NODE",
            Self::Python => "DEEPAGENT_PYTHON",
            Self::Java => "DEEPAGENT_JAVA",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePreference {
    PreferLocal,
    LocalOnly,
    ManagedOnly,
}

impl RuntimePreference {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "prefer_local" | "local" => Some(Self::PreferLocal),
            "local_only" => Some(Self::LocalOnly),
            "managed_only" => Some(Self::ManagedOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequirement {
    pub kind: RuntimeKind,
    pub version: Option<String>,
    pub preference: RuntimePreference,
}

impl RuntimeRequirement {
    pub fn prefer_local(kind: RuntimeKind, version: impl Into<String>) -> Self {
        Self {
            kind,
            version: Some(version.into()),
            preference: RuntimePreference::PreferLocal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSource {
    Local,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResolution {
    pub kind: RuntimeKind,
    pub source: RuntimeSource,
    pub executable: PathBuf,
    pub version: String,
    pub root: PathBuf,
    pub reason: String,
}

/// One runtime probe result returned by [`RuntimeBroker::diagnostics`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDiagnostic {
    pub kind: RuntimeKind,
    pub requirement: String,
    pub resolution: Option<RuntimeResolution>,
    pub error: Option<String>,
}

/// Global runtime resolver shared by every DeepAgent-owned process entry.
///
/// Installation and registry ownership remain in [`RuntimeService`]. The
/// broker only centralizes project-aware resolution and child environments so
/// callers cannot accidentally implement a second runtime selection policy.
#[derive(Clone)]
pub struct RuntimeBroker {
    service: Arc<RuntimeService>,
}

impl RuntimeBroker {
    pub fn new(service: Arc<RuntimeService>) -> Self {
        Self { service }
    }

    pub fn service(&self) -> &Arc<RuntimeService> {
        &self.service
    }

    pub fn resolve(
        &self,
        requirement: &RuntimeRequirement,
        project_root: Option<&Path>,
        explicit: Option<&Path>,
    ) -> Result<RuntimeResolution> {
        let result = self
            .service
            .resolve_runtime(requirement, project_root, explicit);
        match &result {
            Ok(runtime) => tracing::debug!(
                runtime = requirement.kind.command(),
                source = ?runtime.source,
                version = runtime.version,
                executable = %runtime.executable.display(),
                reason = runtime.reason,
                "runtime resolved"
            ),
            Err(error) => tracing::warn!(
                runtime = requirement.kind.command(),
                requirement = requirement.version.as_deref().unwrap_or("any"),
                preference = ?requirement.preference,
                project_root = project_root.map(|path| path.display().to_string()),
                error = %error,
                "runtime resolution failed"
            ),
        }
        result
    }

    pub fn resolve_command(&self, command: &str, project_root: Option<&Path>) -> Result<PathBuf> {
        self.service.resolve_command(command, project_root)
    }

    pub fn build_process_environment(
        &self,
        project_root: Option<&Path>,
    ) -> BTreeMap<String, String> {
        self.service.build_process_environment(project_root)
    }

    pub fn diagnostics(&self, project_root: Option<&Path>) -> Vec<RuntimeDiagnostic> {
        [
            RuntimeRequirement::prefer_local(RuntimeKind::Node, ">=20.19"),
            RuntimeRequirement::prefer_local(RuntimeKind::Python, ">=3.11"),
            RuntimeRequirement::prefer_local(RuntimeKind::Java, ">=17"),
        ]
        .into_iter()
        .map(
            |requirement| match self.resolve(&requirement, project_root, None) {
                Ok(resolution) => RuntimeDiagnostic {
                    kind: requirement.kind,
                    requirement: requirement.version.unwrap_or_default(),
                    resolution: Some(resolution),
                    error: None,
                },
                Err(error) => RuntimeDiagnostic {
                    kind: requirement.kind,
                    requirement: requirement.version.unwrap_or_default(),
                    resolution: None,
                    error: Some(error.to_string()),
                },
            },
        )
        .collect()
    }
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

        // 1. Check managed runtime directories first. On-demand resources
        // should prefer the app-controlled runtime directory over any manual
        // system install.
        if let Some(artifact) = entry.artifact() {
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
                if artifact.archive == ArchiveKind::Installer {
                    if let Some(parent) = dir.parent() {
                        let parent_probe =
                            parent.join(artifact.probe.as_deref().unwrap_or(&entry.probe));
                        if parent_probe.exists() {
                            let source = if same_path(root, &self.active_root) {
                                "active"
                            } else {
                                "fallback"
                            };
                            return Some((parent.to_path_buf(), source));
                        }
                    }
                }
            }
        }

        // 2. Check system probe paths (winget / brew / apt / manual installs).
        if !entry.system_probe_paths.is_empty() {
            if let Some(dir) = find_in_system_paths(&entry.probe, &entry.system_probe_paths) {
                return Some((dir, "system"));
            }
        }

        // 3. Check PATH lookup (works for any installed binary).
        if let Some(dir) = find_binary_on_path(&entry.probe) {
            return Some((dir, "system"));
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

    /// Resolve an SDK executable. Project and PATH runtimes win by default;
    /// managed runtimes are a compatibility fallback and never alter the
    /// user's machine-level environment.
    pub fn resolve_runtime(
        &self,
        requirement: &RuntimeRequirement,
        project_root: Option<&Path>,
        explicit: Option<&Path>,
    ) -> Result<RuntimeResolution> {
        let mut rejected = Vec::new();
        if requirement.preference != RuntimePreference::ManagedOnly {
            if let Some(path) = explicit {
                if let Some(found) = inspect_runtime(requirement.kind, path, &requirement.version) {
                    return Ok(resolution(
                        found,
                        requirement.kind,
                        RuntimeSource::Local,
                        "explicit executable",
                    ));
                }
                rejected.push(format!(
                    "explicit executable '{}' is unavailable or incompatible",
                    path.display()
                ));
            }
            if let Some(root) = project_root {
                for candidate in project_runtime_candidates(requirement.kind, root) {
                    if let Some(found) =
                        inspect_runtime(requirement.kind, &candidate, &requirement.version)
                    {
                        return Ok(resolution(
                            found,
                            requirement.kind,
                            RuntimeSource::Local,
                            "project runtime",
                        ));
                    }
                }
            }
            if let Some(path) = find_executable_on_path(requirement.kind.command()) {
                if let Some(found) = inspect_runtime(requirement.kind, &path, &requirement.version)
                {
                    return Ok(resolution(
                        found,
                        requirement.kind,
                        RuntimeSource::Local,
                        "user PATH",
                    ));
                }
                rejected.push(format!(
                    "{} on PATH does not satisfy the version requirement",
                    path.display()
                ));
            }
        }

        if requirement.preference != RuntimePreference::LocalOnly {
            if let Some(path) = self.managed_runtime_executable(requirement.kind) {
                if let Some(found) = inspect_runtime(requirement.kind, &path, &requirement.version)
                {
                    return Ok(resolution(
                        found,
                        requirement.kind,
                        RuntimeSource::Managed,
                        "DeepAgent managed fallback",
                    ));
                }
                rejected.push(format!("managed {} is incompatible", path.display()));
            }
        }

        let detail = if rejected.is_empty() {
            String::new()
        } else {
            format!(": {}", rejected.join("; "))
        };
        Err(CoreError::Other(format!(
            "no compatible {} runtime was found{}",
            requirement.kind.command(),
            detail
        )))
    }

    /// Resolve a well-known command to an absolute executable path.
    pub fn resolve_command(&self, command: &str, project_root: Option<&Path>) -> Result<PathBuf> {
        let kind = match command.trim().to_ascii_lowercase().as_str() {
            "node" | "node.exe" => RuntimeKind::Node,
            "python" | "python.exe" | "python3" => RuntimeKind::Python,
            "java" | "java.exe" => RuntimeKind::Java,
            other => {
                return find_executable_on_path(other).ok_or_else(|| {
                    CoreError::Other(format!("command '{command}' was not found on PATH"))
                })
            }
        };
        self.resolve_runtime(
            &RuntimeRequirement {
                kind,
                version: None,
                preference: RuntimePreference::PreferLocal,
            },
            project_root,
            None,
        )
        .map(|runtime| runtime.executable)
    }

    /// Environment inherited by every DeepAgent-owned local process. Existing
    /// PATH entries remain first; managed SDK directories are appended only as
    /// fallbacks. Absolute executable variables make plugin launch deterministic.
    pub fn build_process_environment(
        &self,
        project_root: Option<&Path>,
    ) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        let requirements = [
            RuntimeRequirement::prefer_local(RuntimeKind::Node, ">=20.19"),
            RuntimeRequirement::prefer_local(RuntimeKind::Python, ">=3.11"),
            RuntimeRequirement::prefer_local(RuntimeKind::Java, ">=17"),
        ];
        let mut project_bins = Vec::new();
        let mut managed_bins = Vec::new();
        let mut sources = Vec::new();
        for requirement in requirements {
            if let Ok(runtime) = self.resolve_runtime(&requirement, project_root, None) {
                env.insert(
                    requirement.kind.env_key().to_string(),
                    runtime.executable.display().to_string(),
                );
                sources.push(match runtime.source {
                    RuntimeSource::Local => "local",
                    RuntimeSource::Managed => "managed",
                });
                if let Some(parent) = runtime.executable.parent() {
                    if runtime.reason == "project runtime" {
                        project_bins.push(parent.to_path_buf());
                    } else if runtime.source == RuntimeSource::Managed {
                        managed_bins.push(parent.to_path_buf());
                    }
                }
            }
        }
        let mut paths = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .unwrap_or_default();
        for path in project_bins.into_iter().rev() {
            if !paths.iter().any(|existing| same_path(existing, &path)) {
                paths.insert(0, path);
            }
        }
        for path in managed_bins {
            if !paths.iter().any(|existing| same_path(existing, &path)) {
                paths.push(path);
            }
        }
        if let Ok(path) = std::env::join_paths(paths) {
            env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
        }
        env.insert(
            "DEEPAGENT_RUNTIME_ROOT".to_string(),
            self.active_root.display().to_string(),
        );
        env.insert(
            "DEEPAGENT_RUNTIME_SOURCE".to_string(),
            if sources.is_empty() {
                "unavailable"
            } else if sources.iter().all(|source| *source == "local") {
                "local"
            } else if sources.iter().all(|source| *source == "managed") {
                "managed"
            } else {
                "mixed"
            }
            .to_string(),
        );
        if let Some(root) = project_root {
            env.insert(
                "DEEPAGENT_PROJECT_ROOT".to_string(),
                root.display().to_string(),
            );
        }
        env
    }

    fn managed_runtime_executable(&self, kind: RuntimeKind) -> Option<PathBuf> {
        let entry = self
            .registry
            .iter()
            .find(|entry| entry.id == kind.managed_id())?;
        let probe = entry
            .artifact()
            .and_then(|artifact| artifact.probe.as_deref())
            .unwrap_or(&entry.probe);
        let artifact = entry.artifact()?;
        self.lookup_roots.iter().find_map(|root| {
            let executable = root.join(&artifact.dest_subdir).join(probe);
            executable.is_file().then_some(executable)
        })
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

        // Installer-type artifacts are always available (no pinned checksum
        // required — the installer self-verifies). Other artifacts need a
        // pinned SHA-256 to be installable.
        let has_installer = artifact
            .map(|a| a.archive == ArchiveKind::Installer)
            .unwrap_or(false);

        let available_for_platform = artifact.is_some();
        let checksum_pinned = if has_installer {
            true
        } else {
            artifact.map(artifact_integrity_pinned).unwrap_or(false)
        };

        RuntimeStatusDto {
            id: entry.id.clone(),
            name: entry.name.clone(),
            version: entry.version.clone(),
            capability: entry.capability.clone(),
            size_bytes: entry.size_bytes,
            installed: installed.is_some(),
            available_for_platform,
            checksum_pinned,
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
                ArchiveKind::Zip | ArchiveKind::TarGz | ArchiveKind::Installer => {
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
        Ok(true)
    }

    /// Download + verify + install the runtime `id`. Reports progress through
    /// `progress` (downloaded/total). Fails closed: no pinned checksum ⇒ error;
    /// checksum mismatch ⇒ the temp file is removed and an error returned.
    ///
    /// For [`ArchiveKind::Installer`] artifacts, the downloaded installer is
    /// run silently into the managed runtime directory (same as other
    /// resources). SHA-256 is optional for installers because the installer
    /// itself verifies integrity.
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
        // For installer-type artifacts, skip the SHA-256 requirement (the
        // installer verifies its own integrity) and run silently into the
        // managed runtime directory.
        let is_installer = artifact.archive == ArchiveKind::Installer;

        let expected = artifact.sha256.clone();
        if !is_installer && expected.is_none() {
            return Err(CoreError::Other(format!(
                "'{id}' has no pinned SHA-256 — refusing to install unverified runtime"
            )));
        }

        std::fs::create_dir_all(&self.active_root)
            .map_err(|e| CoreError::Other(format!("create runtimes dir: {e}")))?;
        let tmp = download_temp_path(&self.active_root, id, &artifact);

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

        // Verify (skip for installers — they self-verify).
        if let Some(ref expected_hash) = expected {
            let actual = match sha256_file(&tmp) {
                Ok(h) => h,
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    self.clear_cancel(id);
                    return Err(e);
                }
            };
            if actual != expected_hash.to_lowercase() {
                let _ = std::fs::remove_file(&tmp);
                self.clear_cancel(id);
                return Err(CoreError::Other(format!(
                    "checksum mismatch for '{id}': expected {expected_hash}, got {actual}"
                )));
            }
        }

        // Install.
        let dest_dir = self.active_root.join(&artifact.dest_subdir);
        let install_result = match artifact.archive {
            ArchiveKind::Zip => extract_zip_into(&tmp, &dest_dir),
            ArchiveKind::TarGz => extract_tar_gz_into(&tmp, &dest_dir),
            ArchiveKind::Raw => copy_raw(&tmp, &dest_dir, &artifact.file_name),
            ArchiveKind::Installer => run_installer_silent(&tmp, &dest_dir),
        };
        let _ = std::fs::remove_file(&tmp);
        self.clear_cancel(id);
        install_result?;
        if is_installer {
            let probe = dest_dir.join(artifact.probe.as_deref().unwrap_or(&entry.probe));
            if !probe.is_file() {
                return Err(CoreError::Other(format!(
                    "installer finished but runtime probe was not found: {}",
                    probe.display()
                )));
            }
        }

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

struct InspectedRuntime {
    executable: PathBuf,
    version: String,
}

fn resolution(
    found: InspectedRuntime,
    kind: RuntimeKind,
    source: RuntimeSource,
    reason: &str,
) -> RuntimeResolution {
    let root = found
        .executable
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    RuntimeResolution {
        kind,
        source,
        executable: found.executable,
        version: found.version,
        root,
        reason: reason.to_string(),
    }
}

fn inspect_runtime(
    kind: RuntimeKind,
    executable: &Path,
    requirement: &Option<String>,
) -> Option<InspectedRuntime> {
    if !executable.is_file() {
        return None;
    }
    let mut command = std::process::Command::new(executable);
    command.arg("--version");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output().ok()?;
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let version = extract_version(&text)?;
    if !version_matches(&version, requirement.as_deref()) {
        return None;
    }
    let _ = kind;
    Some(InspectedRuntime {
        executable: executable.to_path_buf(),
        version,
    })
}

fn extract_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find_map(|part| {
            let trimmed = part.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
            let starts_numeric = trimmed
                .as_bytes()
                .first()
                .map(|byte| byte.is_ascii_digit())
                .unwrap_or(false);
            starts_numeric.then(|| trimmed.trim_end_matches('.').to_string())
        })
        .filter(|version| !version.is_empty())
}

fn version_matches(version: &str, requirement: Option<&str>) -> bool {
    let Some(requirement) = requirement.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let (operator, expected) = if let Some(value) = requirement.strip_prefix(">=") {
        (">=", value.trim())
    } else if let Some(value) = requirement.strip_prefix('=') {
        ("=", value.trim())
    } else {
        ("prefix", requirement)
    };
    let actual = numeric_version(version);
    let expected_numeric = numeric_version(expected);
    match operator {
        ">=" => compare_versions(&actual, &expected_numeric) != std::cmp::Ordering::Less,
        "=" => compare_versions(&actual, &expected_numeric) == std::cmp::Ordering::Equal,
        _ => actual.starts_with(&expected_numeric),
    }
}

fn numeric_version(version: &str) -> Vec<u32> {
    version
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or_default()
        })
        .collect()
}

fn compare_versions(left: &[u32], right: &[u32]) -> std::cmp::Ordering {
    let count = left.len().max(right.len());
    (0..count)
        .map(|index| {
            left.get(index)
                .copied()
                .unwrap_or_default()
                .cmp(&right.get(index).copied().unwrap_or_default())
        })
        .find(|ordering| *ordering != std::cmp::Ordering::Equal)
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn project_runtime_candidates(kind: RuntimeKind, root: &Path) -> Vec<PathBuf> {
    let names: &[&str] = match kind {
        RuntimeKind::Node => {
            #[cfg(target_os = "windows")]
            {
                &["node.exe"]
            }
            #[cfg(not(target_os = "windows"))]
            {
                &["node"]
            }
        }
        RuntimeKind::Python => {
            #[cfg(target_os = "windows")]
            {
                &[".venv/Scripts/python.exe", "venv/Scripts/python.exe"]
            }
            #[cfg(not(target_os = "windows"))]
            {
                &[".venv/bin/python", "venv/bin/python"]
            }
        }
        RuntimeKind::Java => {
            #[cfg(target_os = "windows")]
            {
                &[".jdk/bin/java.exe", "jdk/bin/java.exe"]
            }
            #[cfg(not(target_os = "windows"))]
            {
                &[".jdk/bin/java", "jdk/bin/java"]
            }
        }
    };
    names.iter().map(|name| root.join(name)).collect()
}

fn find_executable_on_path(command: &str) -> Option<PathBuf> {
    let command_path = Path::new(command);
    if command_path.is_absolute() && command_path.is_file() {
        return Some(command_path.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    let extensions: Vec<OsString> = if cfg!(target_os = "windows") {
        if Path::new(command).extension().is_some() {
            vec![OsString::new()]
        } else {
            std::env::var_os("PATHEXT")
                .map(|value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .filter(|ext| !ext.is_empty())
                        .map(OsString::from)
                        .collect()
                })
                .unwrap_or_else(|| vec![OsString::from(".EXE"), OsString::from(".CMD")])
        }
    } else {
        vec![OsString::new()]
    };
    for dir in std::env::split_paths(&paths) {
        for extension in &extensions {
            let mut file = OsString::from(command);
            file.push(extension);
            let candidate = dir.join(file);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Run an installer silently into `dest_dir` (the managed runtime directory).
///
/// NSIS-based installers use `/S` for silent mode and `/D=...` for the target
/// directory, with `/D` as the final argument. We intentionally do not elevate
/// with `runas`: resource installs must stay background-only and must not show
/// UAC or installer wizard prompts.
fn run_installer_silent(installer: &Path, dest_dir: &Path) -> Result<()> {
    if !cfg!(target_os = "windows") {
        return Err(CoreError::Other(
            "installer-type artifacts are only supported on Windows".to_string(),
        ));
    }
    std::fs::create_dir_all(dest_dir)
        .map_err(|e| CoreError::Other(format!("create dest dir: {e}")))?;

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let installer = shell_execute_path(installer);
        let dest = shell_execute_path(dest_dir);
        let status = std::process::Command::new(&installer)
            .arg("/S")
            // NSIS requires /D to be the last argument.
            .arg(format!("/D={}", dest.to_string_lossy()))
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| {
                if e.raw_os_error() == Some(740) {
                    CoreError::Other(
                        "silent installer requires elevation; use a portable runtime package or an installer that supports per-user silent install"
                            .to_string(),
                    )
                } else {
                    CoreError::Other(format!("run silent installer: {e}"))
                }
            })?;

        if !status.success() {
            return Err(CoreError::Other(format!(
                "silent installer exited with status {status}"
            )));
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn run_elevated_installer(installer: &Path, args: &str) -> Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    // SHELLEXECUTEINFOW structure (simplified).
    #[repr(C)]
    struct ShellExecuteInfoW {
        cb_size: u32,
        f_mask: u32,
        hwnd: *mut u32, // HWND (we pass null)
        lp_verb: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show: i32,
        h_inst_app: *mut u32, // HINSTANCE
        lp_id_list: *mut u32,
        lp_class: *const u16,
        hkey_class: *mut u32,
        dw_hot_key: u32,
        h_icon_or_monitor: *mut u32,
        h_process: *mut u32, // HANDLE
    }

    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x00000040;
    const SW_HIDE: i32 = 0;
    const INFINITE: u32 = 0xFFFFFFFF;
    const WAIT_OBJECT_0: u32 = 0;
    const STILL_ACTIVE: u32 = 259;

    extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
        fn WaitForSingleObject(handle: *mut u32, ms: u32) -> u32;
        fn GetExitCodeProcess(handle: *mut u32, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut u32) -> i32;
    }

    fn to_wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    let shell_installer = shell_execute_path(installer);
    let installer_wide = to_wide(shell_installer.as_os_str());
    let args_wide = to_wide(OsStr::new(args));
    let directory_wide = shell_installer
        .parent()
        .map(|dir| to_wide(dir.as_os_str()))
        .unwrap_or_else(|| vec![0]);
    let verb_wide: Vec<u16> = "runas\0".encode_utf16().collect();

    let mut info = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: ptr::null_mut(),
        lp_verb: verb_wide.as_ptr(),
        lp_file: installer_wide.as_ptr(),
        lp_parameters: args_wide.as_ptr(),
        lp_directory: directory_wide.as_ptr(),
        n_show: SW_HIDE,
        h_inst_app: ptr::null_mut(),
        lp_id_list: ptr::null_mut(),
        lp_class: ptr::null(),
        hkey_class: ptr::null_mut(),
        dw_hot_key: 0,
        h_icon_or_monitor: ptr::null_mut(),
        h_process: ptr::null_mut(),
    };

    let success = unsafe { ShellExecuteExW(&mut info) };
    if success == 0 {
        return Err(CoreError::Other(
            "failed to launch installer with elevation (UAC prompt may have been declined)"
                .to_string(),
        ));
    }

    if info.h_process.is_null() {
        // No process handle — nothing to wait for.
        return Ok(());
    }

    // Wait for the installer to finish.
    let wait_result = unsafe { WaitForSingleObject(info.h_process, INFINITE) };

    if wait_result != WAIT_OBJECT_0 {
        let _ = unsafe { CloseHandle(info.h_process) };
        return Err(CoreError::Other(
            "installer process wait failed unexpectedly".to_string(),
        ));
    }

    // Check exit code.
    let mut exit_code: u32 = 0;
    let got_code = unsafe { GetExitCodeProcess(info.h_process, &mut exit_code) };
    let _ = unsafe { CloseHandle(info.h_process) };
    // If we couldn't get the code, treat as success (installer ran to completion).
    if got_code != 0 && exit_code != 0 && exit_code != STILL_ACTIVE {
        return Err(CoreError::Other(format!(
            "installer exited with code {exit_code}"
        )));
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn shell_execute_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(stripped) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

/// Search for a binary on the system PATH by iterating the PATH environment
/// variable directly (no subprocess — avoids console window flashing on
/// Windows). Returns the parent directory of the found binary.
fn find_binary_on_path(binary_name: &str) -> Option<PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(binary_name);
        if candidate.is_file() {
            return Some(dir);
        }
        // On Windows also try with `.exe` extension if not already present.
        #[cfg(target_os = "windows")]
        if !binary_name.ends_with(".exe") {
            let candidate_exe = dir.join(format!("{}.exe", binary_name));
            if candidate_exe.is_file() {
                return Some(dir);
            }
        }
    }
    None
}

/// Search for a binary in a list of explicit filesystem paths.
/// Returns the first directory that contains the probe file.
fn find_in_system_paths(probe: &str, paths: &[String]) -> Option<PathBuf> {
    for path_str in paths {
        let dir = expand_system_probe_path(path_str);
        if dir.join(probe).is_file() {
            return Some(dir);
        }
    }
    None
}

fn expand_system_probe_path(path: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(rest) = path.strip_prefix("%LOCALAPPDATA%") {
            if let Some(base) = std::env::var_os("LOCALAPPDATA") {
                return PathBuf::from(base).join(rest.trim_start_matches(['\\', '/']));
            }
        }
        if let Some(rest) = path.strip_prefix("%ProgramFiles%") {
            if let Some(base) = std::env::var_os("ProgramFiles") {
                return PathBuf::from(base).join(rest.trim_start_matches(['\\', '/']));
            }
        }
        if let Some(rest) = path.strip_prefix("%ProgramFiles(x86)%") {
            if let Some(base) = std::env::var_os("ProgramFiles(x86)") {
                return PathBuf::from(base).join(rest.trim_start_matches(['\\', '/']));
            }
        }
    }

    PathBuf::from(path)
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

fn download_temp_path(root: &Path, id: &str, artifact: &RuntimeArtifact) -> PathBuf {
    if artifact.archive == ArchiveKind::Installer {
        let installer_name = Path::new(&artifact.file_name)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("installer.exe");
        return root.join(format!(".dl-{id}-{installer_name}"));
    }
    root.join(format!(".dl-{id}.tmp"))
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    let node_artifacts = per_platform_pinned([
        (
            Platform::WindowsX64,
            "https://nodejs.org/dist/v22.23.2/node-v22.23.2-win-x64.zip",
            &[] as &[&str],
            "1177b4137ba5adaa56354ae40f1080c7450e8ae09cecb47da459d1c52ac99f97",
            "sdk/node-22",
            "node-v22.23.2-win-x64.zip",
            ArchiveKind::Zip,
            "node-v22.23.2-win-x64/node.exe",
        ),
        (
            Platform::WindowsArm64,
            "https://nodejs.org/dist/v22.23.2/node-v22.23.2-win-arm64.zip",
            &[],
            "fec025a6da31757e3b6af84c5a1628e9d38442ca99a2161091d78f2fcfa35ef3",
            "sdk/node-22",
            "node-v22.23.2-win-arm64.zip",
            ArchiveKind::Zip,
            "node-v22.23.2-win-arm64/node.exe",
        ),
        (
            Platform::MacOsX64,
            "https://nodejs.org/dist/v22.23.2/node-v22.23.2-darwin-x64.tar.gz",
            &[],
            "58e99022c2ff89395576cc7fd4d98cea24bb68081475d5f88b801ee8729fb026",
            "sdk/node-22",
            "node-v22.23.2-darwin-x64.tar.gz",
            ArchiveKind::TarGz,
            "node-v22.23.2-darwin-x64/bin/node",
        ),
        (
            Platform::MacOsArm64,
            "https://nodejs.org/dist/v22.23.2/node-v22.23.2-darwin-arm64.tar.gz",
            &[],
            "61130f394c1630d211dd50aecc4353d379480f36d3ac913cd85dbba1aed585c6",
            "sdk/node-22",
            "node-v22.23.2-darwin-arm64.tar.gz",
            ArchiveKind::TarGz,
            "node-v22.23.2-darwin-arm64/bin/node",
        ),
        (
            Platform::LinuxX64,
            "https://nodejs.org/dist/v22.23.2/node-v22.23.2-linux-x64.tar.gz",
            &[],
            "b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a",
            "sdk/node-22",
            "node-v22.23.2-linux-x64.tar.gz",
            ArchiveKind::TarGz,
            "node-v22.23.2-linux-x64/bin/node",
        ),
        (
            Platform::LinuxArm64,
            "https://nodejs.org/dist/v22.23.2/node-v22.23.2-linux-arm64.tar.gz",
            &[],
            "013b59cfd2819703a6f4a14ab891fc46fc2a4e3f5bcd92de3fb4929b43e35b30",
            "sdk/node-22",
            "node-v22.23.2-linux-arm64.tar.gz",
            ArchiveKind::TarGz,
            "node-v22.23.2-linux-arm64/bin/node",
        ),
    ]);
    let python_artifacts = per_platform_pinned([(
        Platform::WindowsX64,
        "https://www.python.org/ftp/python/3.11.9/python-3.11.9-embed-amd64.zip",
        &[] as &[&str],
        "009d6bf7e3b2ddca3d784fa09f90fe54336d5b60f0e0f305c37f400bf83cfd3b",
        "sdk/python-3.11",
        "python-3.11.9-embed-amd64.zip",
        ArchiveKind::Zip,
        "python.exe",
    )]);
    let jdk_artifacts = per_platform_pinned([
        (Platform::WindowsX64, "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.20%2B8/OpenJDK17U-jdk_x64_windows_hotspot_17.0.20_8.zip", &[] as &[&str], "418497be5cf585bdd2203d6486a565d66d3f5e992d5630d45104cb873fab8122", "sdk/jdk-17", "OpenJDK17U-jdk_x64_windows_hotspot_17.0.20_8.zip", ArchiveKind::Zip, "jdk-17.0.20+8/bin/java.exe"),
        (Platform::MacOsX64, "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.20%2B8/OpenJDK17U-jdk_x64_mac_hotspot_17.0.20_8.tar.gz", &[], "3710c3131c5d7c090582b357f1310133a90bf701183d065223f1a0b90b9ed5ae", "sdk/jdk-17", "OpenJDK17U-jdk_x64_mac_hotspot_17.0.20_8.tar.gz", ArchiveKind::TarGz, "jdk-17.0.20+8/Contents/Home/bin/java"),
        (Platform::MacOsArm64, "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.20%2B8/OpenJDK17U-jdk_aarch64_mac_hotspot_17.0.20_8.tar.gz", &[], "524850138c742324fb21fca4ff6ef68ea25f25bf59366a864e45b4a0c45ed0df", "sdk/jdk-17", "OpenJDK17U-jdk_aarch64_mac_hotspot_17.0.20_8.tar.gz", ArchiveKind::TarGz, "jdk-17.0.20+8/Contents/Home/bin/java"),
        (Platform::LinuxX64, "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.20%2B8/OpenJDK17U-jdk_x64_linux_hotspot_17.0.20_8.tar.gz", &[], "be7668bc030d578b83d6d5ef9221d6d6729bbbca8cf94a7d52e16ac68b5a5a35", "sdk/jdk-17", "OpenJDK17U-jdk_x64_linux_hotspot_17.0.20_8.tar.gz", ArchiveKind::TarGz, "jdk-17.0.20+8/bin/java"),
        (Platform::LinuxArm64, "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.20%2B8/OpenJDK17U-jdk_aarch64_linux_hotspot_17.0.20_8.tar.gz", &[], "d143936f473a4cb24e3b0e247d6d0775769d55ec9775c339540e753059a8d77a", "sdk/jdk-17", "OpenJDK17U-jdk_aarch64_linux_hotspot_17.0.20_8.tar.gz", ArchiveKind::TarGz, "jdk-17.0.20+8/bin/java"),
    ]);

    vec![
        RuntimeEntry {
            id: "node-22".to_string(),
            name: "Node.js".to_string(),
            version: "22.23.2".to_string(),
            capability: "node".to_string(),
            size_bytes: 55 * 1024 * 1024,
            artifacts: node_artifacts,
            probe: if cfg!(target_os = "windows") { "node.exe" } else { "node" }.to_string(),
            system_probe_paths: vec![],
        },
        RuntimeEntry {
            id: "python-3.11".to_string(),
            name: "Python".to_string(),
            version: "3.11.9".to_string(),
            capability: "python".to_string(),
            size_bytes: 11 * 1024 * 1024,
            artifacts: python_artifacts,
            probe: if cfg!(target_os = "windows") { "python.exe" } else { "python" }.to_string(),
            system_probe_paths: vec![],
        },
        RuntimeEntry {
            id: "jdk-17".to_string(),
            name: "Eclipse Temurin JDK".to_string(),
            version: "17.0.20+8".to_string(),
            capability: "java".to_string(),
            size_bytes: 190 * 1024 * 1024,
            artifacts: jdk_artifacts,
            probe: if cfg!(target_os = "windows") { "java.exe" } else { "java" }.to_string(),
            system_probe_paths: vec![],
        },
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
            system_probe_paths: vec![],
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
            system_probe_paths: vec![],
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
            system_probe_paths: vec![],
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
            system_probe_paths: vec![
                "C:\\Program Files\\Pandoc".to_string(),
            ],
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
            system_probe_paths: vec![],
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
            system_probe_paths: vec![
                "C:\\Program Files\\LibreOffice\\program".to_string(),
            ],
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
            system_probe_paths: vec![],
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
            system_probe_paths: vec![],
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
            system_probe_paths: vec![],
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
            system_probe_paths: vec![],
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
        // Tier R binaries: pandoc / pdfium / libreoffice are fail-closed
        // until a checksum is pinned (no winget_id fallback on this platform).
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
