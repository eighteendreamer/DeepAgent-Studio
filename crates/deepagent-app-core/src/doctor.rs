//! Environment diagnostics for `/doctor`.
//!
//! Inspired by Claude Code's doctor flow: keep each check small, structured,
//! and paired with a concrete repair hint so the UI can render a scan-friendly
//! green/yellow/red report.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

use crate::settings::{SettingsService, WebSearchProvider, WebSearchSettings};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Severity for one diagnostic check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagStatus {
    /// Check passed.
    Ok,
    /// Check is degraded or unavailable but not necessarily blocking.
    Warning,
    /// Check failed and likely blocks normal agent use.
    Error,
}

/// One structured diagnostic result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticResult {
    /// Stable display name.
    pub name: String,
    /// Green/yellow/red status.
    pub status: DiagStatus,
    /// Short diagnosis.
    pub detail: String,
    /// Optional actionable repair hint.
    pub fix_hint: Option<String>,
}

impl DiagnosticResult {
    fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: DiagStatus::Ok,
            detail: detail.into(),
            fix_hint: None,
        }
    }

    fn warning(
        name: impl Into<String>,
        detail: impl Into<String>,
        fix_hint: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: DiagStatus::Warning,
            detail: detail.into(),
            fix_hint: Some(fix_hint.into()),
        }
    }

    fn error(
        name: impl Into<String>,
        detail: impl Into<String>,
        fix_hint: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: DiagStatus::Error,
            detail: detail.into(),
            fix_hint: Some(fix_hint.into()),
        }
    }
}

/// Run the full doctor suite.
pub async fn run_diagnostics(
    settings: &SettingsService,
    db: &Database,
    workspace_root: &Path,
    app_data_dir: &Path,
) -> Vec<DiagnosticResult> {
    let mut results = Vec::with_capacity(7);
    results.push(check_api_key(settings).await);
    results.push(check_network().await);
    results.push(check_web_search(settings));
    results.push(check_git());
    results.push(check_workspace_permissions(workspace_root));
    results.push(check_database(db));
    results.push(check_disk_space(app_data_dir));
    results
}

fn check_web_search(settings: &SettingsService) -> DiagnosticResult {
    match settings.web_search_settings() {
        Ok(config) => diagnose_web_search_config(config),
        Err(e) => DiagnosticResult::warning(
            "Web search",
            format!("could not read web-search settings: {e}"),
            "Initialize settings, then choose a web-search provider in Settings.",
        ),
    }
}

fn diagnose_web_search_config(config: WebSearchSettings) -> DiagnosticResult {
    if !config.enabled {
        return DiagnosticResult::warning(
            "Web search",
            "web_search is disabled; web_fetch remains available",
            "Enable Web Search in Settings when current web results are needed.",
        );
    }
    match config.provider {
        WebSearchProvider::DeepSeekFirst => {
            let fallback = config
                .searxng_url
                .as_deref()
                .map(|url| format!("; SearXNG fallback: {url}"))
                .unwrap_or_else(|| "; no SearXNG fallback configured".to_string());
            DiagnosticResult::ok("Web search", format!("provider deepseek_first{fallback}"))
        }
        WebSearchProvider::Searxng => match config.searxng_url.as_deref() {
            Some(url) if !url.trim().is_empty() => {
                DiagnosticResult::ok("Web search", format!("provider searxng; bridge: {url}"))
            }
            _ => DiagnosticResult::warning(
                "Web search",
                "provider searxng is selected but no SearXNG URL is configured",
                "Set a SearXNG bridge URL in Settings or switch provider to DeepSeek first.",
            ),
        },
        WebSearchProvider::DuckDuckGo => DiagnosticResult::warning(
            "Web search",
            "provider duckduckgo uses keyless HTML scraping only",
            "Prefer DeepSeek first or a SearXNG bridge for better reliability.",
        ),
    }
}

/// Render diagnostics into the slash-command acknowledgement.
pub fn format_diagnostics(results: &[DiagnosticResult]) -> String {
    let mut out = String::from("Doctor diagnostics:\n");
    for result in results {
        let mark = match result.status {
            DiagStatus::Ok => "[OK]",
            DiagStatus::Warning => "[WARN]",
            DiagStatus::Error => "[ERROR]",
        };
        out.push_str(&format!("- {mark} {}: {}", result.name, result.detail));
        if let Some(hint) = &result.fix_hint {
            out.push_str(&format!(" Fix: {hint}"));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

async fn check_api_key(settings: &SettingsService) -> DiagnosticResult {
    match settings.api_key() {
        Ok(Some(key)) if !key.trim().is_empty() => match settings.validate_api_key(&key).await {
            Ok(count) => DiagnosticResult::ok(
                "API Key",
                format!("valid; DeepSeek returned {count} model(s)"),
            ),
            Err(e) => DiagnosticResult::error(
                "API Key",
                format!("configured but validation failed: {e}"),
                "Re-enter a valid DeepSeek API key in Settings.",
            ),
        },
        Ok(_) => DiagnosticResult::error(
            "API Key",
            "not configured",
            "Open Settings and enter a DeepSeek API key.",
        ),
        Err(e) => DiagnosticResult::error(
            "API Key",
            format!("could not read secret store: {e}"),
            "Check encrypted SQLite and OS keychain access, then re-enter the API key.",
        ),
    }
}

async fn check_network() -> DiagnosticResult {
    let connect = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(("api.deepseek.com", 443)),
    )
    .await;
    match connect {
        Ok(Ok(_)) => DiagnosticResult::ok("Network", "api.deepseek.com:443 is reachable"),
        Ok(Err(e)) => DiagnosticResult::error(
            "Network",
            format!("could not connect to api.deepseek.com:443: {e}"),
            "Check network access, DNS, proxy, or firewall settings.",
        ),
        Err(_) => DiagnosticResult::error(
            "Network",
            "connection to api.deepseek.com:443 timed out after 5s",
            "Check network access, DNS, proxy, or firewall settings.",
        ),
    }
}

fn check_git() -> DiagnosticResult {
    let mut cmd = Command::new("git");
    cmd.arg("--version");
    configure_hidden_process(&mut cmd);
    match cmd.output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            DiagnosticResult::ok(
                "Git",
                if version.is_empty() {
                    "available".into()
                } else {
                    version
                },
            )
        }
        Ok(output) => DiagnosticResult::error(
            "Git",
            format!("git --version exited with {}", output.status),
            "Install Git and ensure it is available on PATH.",
        ),
        Err(e) => DiagnosticResult::error(
            "Git",
            format!("git executable was not found or could not run: {e}"),
            "Install Git and ensure it is available on PATH.",
        ),
    }
}

fn check_workspace_permissions(root: &Path) -> DiagnosticResult {
    if let Err(e) = std::fs::create_dir_all(root) {
        return DiagnosticResult::error(
            "Workspace permissions",
            format!("workspace directory is not accessible: {e}"),
            "Choose a writable project folder or fix directory permissions.",
        );
    }
    let path = root.join(format!(".deepagent_doctor_{}.tmp", std::process::id()));
    match std::fs::write(&path, b"doctor") {
        Ok(()) => {
            let _ = std::fs::remove_file(&path);
            DiagnosticResult::ok(
                "Workspace permissions",
                format!("read/write check passed in {}", root.display()),
            )
        }
        Err(e) => DiagnosticResult::error(
            "Workspace permissions",
            format!("could not write {}: {e}", path.display()),
            "Choose a writable project folder or fix directory permissions.",
        ),
    }
}

fn check_database(db: &Database) -> DiagnosticResult {
    match db.with_conn(|conn| {
        let value: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
        Ok(value)
    }) {
        Ok(value) if value == "ok" => DiagnosticResult::ok("Database", "SQLite integrity_check ok"),
        Ok(value) => DiagnosticResult::error(
            "Database",
            format!("SQLite integrity_check returned: {value}"),
            "Back up the app data directory and recreate the database.",
        ),
        Err(e) => DiagnosticResult::error(
            "Database",
            format!("could not run SQLite integrity_check: {e}"),
            "Restart the app; if this repeats, back up and recreate the database.",
        ),
    }
}

fn check_disk_space(app_data_dir: &Path) -> DiagnosticResult {
    match available_space_bytes(app_data_dir) {
        Ok(bytes) if bytes >= 100 * 1024 * 1024 => DiagnosticResult::ok(
            "Disk space",
            format!(
                "{} free near {}",
                format_bytes(bytes),
                app_data_dir.display()
            ),
        ),
        Ok(bytes) => DiagnosticResult::warning(
            "Disk space",
            format!(
                "only {} free near {}",
                format_bytes(bytes),
                app_data_dir.display()
            ),
            "Free at least 100 MB on the app-data volume.",
        ),
        Err(e) => DiagnosticResult::warning(
            "Disk space",
            format!("could not determine free space: {e}"),
            "Ensure the app-data directory is on a healthy writable volume.",
        ),
    }
}

#[cfg(windows)]
fn available_space_bytes(path: &Path) -> Result<u64> {
    let root = drive_root(path).ok_or_else(|| CoreError::other("path has no drive root"))?;
    let script = format!("(Get-PSDrive -Name '{}').Free", root.trim_end_matches(':'));
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", &script]);
    configure_hidden_process(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| CoreError::other(format!("failed to run PowerShell: {e}")))?;
    if !output.status.success() {
        return Err(CoreError::other(format!(
            "PowerShell exited with {}",
            output.status
        )));
    }
    parse_u64_stdout(&output.stdout)
}

#[cfg(not(windows))]
fn available_space_bytes(path: &Path) -> Result<u64> {
    let output = Command::new("df")
        .args(["-Pk", &path.to_string_lossy()])
        .output()
        .map_err(|e| CoreError::other(format!("failed to run df: {e}")))?;
    if !output.status.success() {
        return Err(CoreError::other(format!(
            "df exited with {}",
            output.status
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .nth(1)
        .ok_or_else(|| CoreError::other("df output was missing data row"))?;
    let available_kb = line
        .split_whitespace()
        .nth(3)
        .ok_or_else(|| CoreError::other("df output was missing available column"))?
        .parse::<u64>()
        .map_err(|e| CoreError::other(format!("bad df available column: {e}")))?;
    Ok(available_kb * 1024)
}

#[cfg(windows)]
fn drive_root(path: &Path) -> Option<String> {
    let path = std::path::PathBuf::from(path);
    let mut components = path.components();
    match components.next()? {
        std::path::Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().into()),
        _ => None,
    }
}

#[cfg(windows)]
fn parse_u64_stdout(stdout: &[u8]) -> Result<u64> {
    String::from_utf8_lossy(stdout)
        .trim()
        .parse::<u64>()
        .map_err(|e| CoreError::other(format!("bad free-space output: {e}")))
}

fn configure_hidden_process(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::MemorySecretStore;
    use deepagent_models::transport::MockTransport;
    use std::sync::Arc;

    fn settings_with_models(db: Arc<Database>) -> SettingsService {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},
            {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}
        ]}"#;
        SettingsService::new(
            db,
            Arc::new(MockTransport::with_get_json(body)),
            Arc::new(MemorySecretStore::new()),
        )
    }

    #[test]
    fn formats_statuses_with_hints() {
        let results = vec![
            DiagnosticResult::ok("Git", "git version 2"),
            DiagnosticResult::warning("Disk", "low", "free space"),
            DiagnosticResult::error("API", "missing", "set key"),
        ];
        let text = format_diagnostics(&results);
        assert!(text.contains("[OK] Git"));
        assert!(text.contains("[WARN] Disk"));
        assert!(text.contains("Fix: free space"));
        assert!(text.contains("[ERROR] API"));
    }

    #[test]
    fn database_integrity_check_passes_for_fresh_db() {
        let db = Database::open_in_memory().unwrap();
        let result = check_database(&db);
        assert_eq!(result.status, DiagStatus::Ok);
    }

    #[test]
    fn workspace_permission_check_writes_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let result = check_workspace_permissions(dir.path());
        assert_eq!(result.status, DiagStatus::Ok);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn web_search_diagnostic_warns_when_searxng_url_missing() {
        let result = diagnose_web_search_config(WebSearchSettings {
            enabled: true,
            provider: WebSearchProvider::Searxng,
            searxng_url: None,
            anysearch_enabled: false,
            anysearch_base_url: None,
            anysearch_api_key_configured: false,
        });
        assert_eq!(result.status, DiagStatus::Warning);
        assert!(result.detail.contains("no SearXNG URL"));
    }

    #[test]
    fn web_search_diagnostic_reports_deepseek_first() {
        let result = diagnose_web_search_config(WebSearchSettings::default());
        assert_eq!(result.status, DiagStatus::Ok);
        assert!(result.detail.contains("deepseek_first"));
    }

    #[tokio::test]
    async fn api_key_check_validates_with_models_endpoint() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = settings_with_models(db);
        settings.initialize("sk-test").await.unwrap();
        let result = check_api_key(&settings).await;
        assert_eq!(result.status, DiagStatus::Ok);
    }
}
