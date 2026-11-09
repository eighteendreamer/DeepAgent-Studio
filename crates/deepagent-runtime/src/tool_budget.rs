//! Tool result budgeting and persistence.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use deepagent_tools::ToolOutput;
use sha2::{Digest, Sha256};

const DEFAULT_MAX_TOOL_RESULT_TOKENS: usize = 4_000;
const DEFAULT_PREVIEW_TOKENS: usize = 2_000;
const CHARS_PER_TOKEN: usize = 4;

/// Runtime configuration for model-visible tool result budgets.
#[derive(Debug, Clone)]
pub struct ToolResultBudgetConfig {
    /// Default maximum estimated tokens before a result is truncated.
    pub max_tokens: usize,
    /// Preview size, in estimated tokens, shown after truncation.
    pub preview_tokens: usize,
    /// Optional per-tool maximum estimated tokens.
    pub per_tool_max_tokens: BTreeMap<String, usize>,
    /// Base directory where full tool results are saved.
    pub output_dir: PathBuf,
    /// Legacy temporary-file cleanup preference. Published result references
    /// are durable history assets and are never removed at turn completion.
    pub cleanup_on_run_end: bool,
}

impl Default for ToolResultBudgetConfig {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_TOOL_RESULT_TOKENS,
            preview_tokens: DEFAULT_PREVIEW_TOKENS,
            per_tool_max_tokens: BTreeMap::new(),
            output_dir: std::env::temp_dir()
                .join("deepagent-studio")
                .join("tool_results"),
            cleanup_on_run_end: false,
        }
    }
}

impl ToolResultBudgetConfig {
    /// Return the token limit for `tool_name`.
    pub fn limit_for(&self, tool_name: &str) -> usize {
        self.per_tool_max_tokens
            .get(tool_name)
            .copied()
            .unwrap_or(self.max_tokens)
            .max(1)
    }

    /// Allocate a fresh result identity, even when a provider reuses a call id
    /// within one session. Opaque ids cannot become path components or collide
    /// after lossy sanitization. The final path is published without overwrite.
    pub fn result_path(&self, call_id: &str) -> PathBuf {
        self.session_result_path("", call_id)
    }

    fn session_result_path(&self, session_id: &str, call_id: &str) -> PathBuf {
        self.output_dir.join(format!(
            "tool_result_{}_{}_{}.json",
            identity_hash(session_id),
            identity_hash(call_id),
            uuid::Uuid::new_v4().simple()
        ))
    }
}

pub(crate) struct SavedToolResult {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) byte_size: i64,
    pub(crate) digest: String,
}

/// Provenance travels out-of-band: arbitrary tool JSON containing `saved_path`
/// is not proof that the host created an artifact and must never be indexed or
/// admitted to an automatic deletion list.
pub(crate) struct BudgetedToolResult {
    pub(crate) output: ToolOutput,
    pub(crate) saved: Option<SavedToolResult>,
}

impl BudgetedToolResult {
    pub(crate) fn persistence_failed(&mut self, error: &str) {
        self.saved = None;
        self.output.value["saved_path"] = serde_json::Value::Null;
        self.output.value["persistence_error"] = error.into();
        self.output.value["message"] =
            "output truncated; full output could not be saved; do not repeat side-effecting tools merely to recover output".into();
    }
}

/// Apply the configured budget to a tool output, persisting the original value
/// when the model-visible result would exceed the limit.
pub async fn apply_tool_result_budget(
    config: &ToolResultBudgetConfig,
    session_id: &str,
    tool_name: &str,
    call_id: &str,
    output: ToolOutput,
) -> ToolOutput {
    budget_tool_result(config, session_id, tool_name, call_id, output)
        .await
        .output
}

pub(crate) async fn budget_tool_result(
    config: &ToolResultBudgetConfig,
    session_id: &str,
    tool_name: &str,
    call_id: &str,
    mut output: ToolOutput,
) -> BudgetedToolResult {
    if output.truncated {
        return BudgetedToolResult {
            output,
            saved: None,
        };
    }

    let full_value = if output.content.is_empty() {
        output.value.clone()
    } else {
        serde_json::json!({"value":output.value,"content":output.content})
    };
    let serialized = match serde_json::to_string_pretty(&full_value) {
        Ok(s) => s,
        Err(_) => output.value.to_string(),
    };
    let estimated_tokens = estimate_tokens(&serialized);
    let max_tokens = config.limit_for(tool_name);
    if estimated_tokens <= max_tokens {
        return BudgetedToolResult {
            output,
            saved: None,
        };
    }

    let file_path = config.session_result_path(session_id, call_id);
    let persisted = persist_full_output(&config.output_dir, &file_path, &serialized).await;
    let preview = truncate_chars(
        &serialized,
        config.preview_tokens.max(1).saturating_mul(CHARS_PER_TOKEN),
    );
    output.truncated = true;
    // Text may be offloaded, but a byte-budgeted image reference must not be
    // truncated into JSON or silently discarded by the textual token budget.
    output.content.retain(|part| {
        matches!(
            part,
            deepagent_core::tool_content::ToolResultPart::Image { .. }
        )
    });
    output.value = serde_json::json!({
        "truncated": true,
        "tool": tool_name,
        "call_id": call_id,
        "saved_path": null,
        "original_estimated_tokens": estimated_tokens,
        "max_tokens": max_tokens,
        "preview": preview,
    });
    let mut budgeted = BudgetedToolResult {
        output,
        saved: None,
    };
    match persisted {
        Ok(path) => {
            let id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap()
                .to_owned();
            budgeted.output.value["saved_path"] = path.to_string_lossy().as_ref().into();
            budgeted.output.value["message"] = format!(
                "output truncated; full output saved to {}; use offset to read more when the tool supports it",
                path.display()
            ).into();
            budgeted.saved = Some(SavedToolResult {
                id,
                path,
                byte_size: serialized.len() as i64,
                digest: format!("sha256:{:x}", Sha256::digest(serialized.as_bytes())),
            });
        }
        Err(error) => {
            tracing::warn!(tool = tool_name, call_id, %error, "full tool result persistence failed");
            budgeted.persistence_failed(&error.to_string());
        }
    }
    budgeted
}

/// Extract the saved path from a budgeted output, if one was recorded.
pub fn saved_path(output: &ToolOutput) -> Option<PathBuf> {
    output
        .value
        .get("saved_path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.starts_with('<'))
        .map(PathBuf::from)
}

/// Explicitly discard unpublished temporary files. Published history assets
/// must instead be reclaimed with their owner and reference retention policy.
pub async fn cleanup_tool_result_paths(paths: Vec<PathBuf>) {
    for path in paths {
        if let Err(error) = tokio::fs::remove_file(&path).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), %error, "temporary tool result cleanup failed");
            }
        }
    }
}

fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(CHARS_PER_TOKEN)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str("\n... [output truncated]");
    out
}

async fn persist_full_output(
    dir: &Path,
    file_path: &Path,
    content: &str,
) -> std::io::Result<PathBuf> {
    let dir = dir.to_owned();
    let file_name = file_path
        .file_name()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tool result path has no file name",
            )
        })?
        .to_owned();
    let content = content.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        // Publish and return a physical absolute root, not an unresolved alias
        // that can later be retargeted. Do not advertise an incomplete file.
        let dir = std::fs::canonicalize(dir)?;
        let path = model_readable_path(dir.join(file_name));
        if path.to_str().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tool result path is not UTF-8",
            ));
        }
        let mut staging = tempfile::NamedTempFile::new_in(&dir)?;
        staging.write_all(content.as_bytes())?;
        staging.as_file().sync_all()?;
        let file = staging
            .persist_noclobber(&path)
            .map_err(|error| error.error)?;
        // The staged bytes were synced before publication. Do not introduce
        // a fallible post-publication step that loses the published identity.
        drop(file);
        Ok(path)
    })
    .await
    .map_err(std::io::Error::other)?
}

fn model_readable_path(path: PathBuf) -> PathBuf {
    // canonicalize returns a verbatim prefix on Windows. The confined file
    // tools compare ordinary workspace paths lexically before physical I/O.
    // Convert only when a round-trip proves the physical identity unchanged.
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        let normal = text
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| PathBuf::from(format!(r"\\{rest}")))
            .or_else(|| text.strip_prefix(r"\\?\").map(PathBuf::from));
        if let Some(normal) = normal {
            // The file does not exist yet, so compare its existing parent.
            if normal
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .as_deref()
                == path.parent()
            {
                return normal;
            }
        }
    }
    path
}

fn identity_hash(input: &str) -> String {
    format!("{:x}", Sha256::digest(input.as_bytes()))[..16].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn large_output_is_truncated_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let config = ToolResultBudgetConfig {
            max_tokens: 4,
            preview_tokens: 2,
            output_dir: dir.path().to_path_buf(),
            cleanup_on_run_end: false,
            ..Default::default()
        };
        let out = ToolOutput::success(serde_json::json!({
            "content": "abcdefghijklmnopqrstuvwxyz"
        }));

        let out = apply_tool_result_budget(&config, "ses_1", "bash", "call/1", out).await;

        assert!(out.truncated);
        assert_eq!(out.value["truncated"], true);
        assert!(out.value["message"]
            .as_str()
            .unwrap()
            .contains("output truncated"));
        let saved = PathBuf::from(out.value["saved_path"].as_str().unwrap());
        assert!(saved.exists());
        assert_eq!(saved.extension().unwrap(), "json");
        assert!(saved
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("tool_result_"));
        let full = tokio::fs::read_to_string(saved).await.unwrap();
        assert!(full.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    #[tokio::test]
    async fn per_tool_limit_overrides_default() {
        let dir = tempfile::tempdir().unwrap();
        let mut per_tool = BTreeMap::new();
        per_tool.insert("grep".to_string(), 2);
        let config = ToolResultBudgetConfig {
            max_tokens: 10_000,
            preview_tokens: 2,
            per_tool_max_tokens: per_tool,
            output_dir: dir.path().to_path_buf(),
            cleanup_on_run_end: false,
        };
        let out = ToolOutput::success(serde_json::json!({"hits": ["abcdefghi"]}));

        let out = apply_tool_result_budget(&config, "ses_1", "grep", "call_1", out).await;

        assert!(out.truncated);
        assert_eq!(out.value["max_tokens"], 2);
    }

    #[tokio::test]
    async fn cleanup_removes_created_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = ToolResultBudgetConfig {
            output_dir: dir.path().to_path_buf(),
            cleanup_on_run_end: true,
            ..Default::default()
        };
        let file = config.result_path("call");
        tokio::fs::create_dir_all(file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&file, "x").await.unwrap();

        cleanup_tool_result_paths(vec![file.clone()]).await;

        assert!(!file.exists());
    }

    #[tokio::test]
    async fn audit_same_call_id_from_two_runs_does_not_overwrite_saved_result() {
        let dir = tempfile::tempdir().unwrap();
        let config = ToolResultBudgetConfig {
            max_tokens: 2,
            preview_tokens: 1,
            output_dir: dir.path().to_path_buf(),
            cleanup_on_run_end: false,
            ..Default::default()
        };
        let first = apply_tool_result_budget(
            &config,
            "session_a",
            "remote_tool",
            "provider_call_1",
            ToolOutput::success(serde_json::json!({"source":"run-a","tail":"AUDIT_A"})),
        )
        .await;
        let first_path = PathBuf::from(first.value["saved_path"].as_str().unwrap());
        let first_bytes = tokio::fs::read_to_string(&first_path).await.unwrap();

        let second = apply_tool_result_budget(
            &config,
            "session_b",
            "remote_tool",
            "provider_call_1",
            ToolOutput::success(serde_json::json!({"source":"run-b","tail":"AUDIT_B"})),
        )
        .await;
        let second_path = PathBuf::from(second.value["saved_path"].as_str().unwrap());
        let second_bytes = tokio::fs::read_to_string(&second_path).await.unwrap();
        eprintln!(
            "H-36 path collision: first_path={} second_path={} first_has_a={} after_second_has_b={}",
            first_path.display(),
            second_path.display(),
            first_bytes.contains("AUDIT_A"),
            second_bytes.contains("AUDIT_B")
        );

        assert_ne!(
            first_path, second_path,
            "saved result paths must be run-scoped"
        );
        assert!(first_bytes.contains("AUDIT_A"));
        assert!(second_bytes.contains("AUDIT_B"));
        assert_eq!(
            tokio::fs::read_to_string(&first_path).await.unwrap(),
            first_bytes
        );
    }

    #[tokio::test]
    async fn result_identities_are_unique_bounded_and_never_use_raw_path_components() {
        let dir = tempfile::tempdir().unwrap();
        let config = ToolResultBudgetConfig {
            max_tokens: 1,
            preview_tokens: 1,
            output_dir: dir.path().to_owned(),
            ..Default::default()
        };
        let long = "长".repeat(1000);
        let mut paths = std::collections::HashSet::new();
        for (index, id) in ["same", "same", "a/b", "a\\b", "", "CON", "../..", &long]
            .into_iter()
            .enumerate()
        {
            let value =
                serde_json::json!({"marker":format!("result-{index}"), "padding":"x".repeat(100)});
            let expected = serde_json::to_string_pretty(&value).unwrap();
            let budgeted = budget_tool_result(
                &config,
                "../../session",
                "test",
                id,
                ToolOutput::success(value),
            )
            .await;
            let saved = budgeted.saved.unwrap();
            assert!(paths.insert(saved.path.clone()));
            assert_eq!(
                std::fs::canonicalize(saved.path.parent().unwrap()).unwrap(),
                std::fs::canonicalize(dir.path()).unwrap()
            );
            assert!(saved.path.file_name().unwrap().len() < 120);
            assert_eq!(std::fs::read_to_string(&saved.path).unwrap(), expected);
            assert_eq!(saved.byte_size, expected.len() as i64);
            assert_eq!(
                saved.digest,
                format!("sha256:{:x}", Sha256::digest(expected.as_bytes()))
            );
        }
        assert_eq!(paths.len(), 8);
    }

    #[tokio::test]
    async fn failed_save_never_claims_full_output_was_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, "leave me unchanged").unwrap();
        let config = ToolResultBudgetConfig {
            output_dir: blocker.clone(),
            max_tokens: 1,
            preview_tokens: 1,
            ..Default::default()
        };
        let output = apply_tool_result_budget(
            &config,
            "session",
            "test",
            "call",
            ToolOutput::success(serde_json::json!({"content":"a".repeat(100)})),
        )
        .await;
        assert!(
            output.ok,
            "storage failure must not change the underlying tool execution status"
        );
        assert!(output.truncated);
        assert!(saved_path(&output).is_none());
        assert!(output.value["persistence_error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()));
        assert!(!output.value.to_string().contains("full output saved"));
        assert_eq!(
            std::fs::read_to_string(blocker).unwrap(),
            "leave me unchanged"
        );
    }

    #[tokio::test]
    async fn publication_refuses_existing_target_and_discards_staging() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("existing.json");
        std::fs::write(&target, "original bytes").unwrap();
        assert_eq!(
            persist_full_output(dir.path(), &target, "replacement bytes")
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), "original bytes");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn tool_provided_truncation_is_not_proof_of_host_artifact_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let foreign = dir.path().join("foreign.txt");
        std::fs::write(&foreign, "not owned by the budget publisher").unwrap();
        let mut output = ToolOutput::success(serde_json::json!({"saved_path":foreign}));
        output.truncated = true;
        let expected = output.value.clone();
        let config = ToolResultBudgetConfig {
            output_dir: dir.path().join("unused"),
            ..Default::default()
        };
        let result = budget_tool_result(&config, "session", "test", "call", output).await;
        assert!(result.saved.is_none());
        assert_eq!(result.output.value, expected);
        assert!(!config.output_dir.exists());
        assert!(foreign.is_file());
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn saved_reference_is_not_retargeted_with_the_configured_directory_alias() {
        fn link(alias: &Path, target: &Path) {
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, alias).unwrap();
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let output = std::process::Command::new("cmd")
                    .args(["/C", "mklink", "/J"])
                    .arg(alias)
                    .arg(target)
                    .creation_flags(0x0800_0000)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "junction fixture: {output:?}");
            }
        }
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let alias = root.path().join("alias");
        link(&alias, &first);
        let config = ToolResultBudgetConfig {
            output_dir: alias.clone(),
            max_tokens: 1,
            preview_tokens: 1,
            ..Default::default()
        };
        let out = apply_tool_result_budget(
            &config,
            "session",
            "test",
            "call",
            ToolOutput::success(serde_json::json!({"content":"original"})),
        )
        .await;
        let saved = saved_path(&out).unwrap();
        let bytes = std::fs::read(&saved).unwrap();
        assert_eq!(
            std::fs::canonicalize(saved.parent().unwrap()).unwrap(),
            std::fs::canonicalize(&first).unwrap()
        );
        std::fs::rename(&alias, root.path().join("previous-alias")).unwrap();
        link(&alias, &second);
        let next = apply_tool_result_budget(
            &config,
            "session",
            "test",
            "call",
            ToolOutput::success(serde_json::json!({"content":"replacement"})),
        )
        .await;
        assert_eq!(std::fs::read(&saved).unwrap(), bytes);
        assert_eq!(
            std::fs::canonicalize(saved_path(&next).unwrap().parent().unwrap()).unwrap(),
            std::fs::canonicalize(&second).unwrap()
        );
    }

    #[tokio::test]
    async fn small_results_are_unchanged_and_do_not_create_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = ToolResultBudgetConfig {
            output_dir: dir.path().join("unused"),
            ..Default::default()
        };
        let value = serde_json::json!({"content":"small"});
        let output = apply_tool_result_budget(
            &config,
            "session",
            "test",
            "call",
            ToolOutput::success(value.clone()),
        )
        .await;
        assert_eq!(output.value, value);
        assert!(!output.truncated);
        assert!(!config.output_dir.exists());
    }

    #[tokio::test]
    async fn h127_text_parts_are_budgeted_without_truncating_image_references() {
        use deepagent_core::message::MessageAttachment;
        use deepagent_core::tool_content::ToolResultPart;
        let fixture = tempfile::tempdir().unwrap();
        let config = ToolResultBudgetConfig {
            max_tokens: 100,
            preview_tokens: 20,
            output_dir: fixture.path().join("results"),
            ..Default::default()
        };
        let reference = MessageAttachment {
            id: "budgeted-image".into(),
            kind: "image".into(),
            media_type: Some("image/png".into()),
            path: Some("admitted.png".into()),
            sha256: Some("a".repeat(64)),
        };
        let mut original = ToolOutput::success(serde_json::json!({"isError":false}));
        original.content = vec![
            ToolResultPart::Text {
                text: "large tool text ".repeat(4000),
            },
            ToolResultPart::Image {
                attachment: reference.clone(),
            },
        ];
        let output = apply_tool_result_budget(
            &config,
            "session",
            "mcp__test__image",
            "call",
            original.clone(),
        )
        .await;
        assert!(output.truncated);
        assert!(output.ok);
        assert_eq!(
            output.content,
            vec![ToolResultPart::Image {
                attachment: reference
            }]
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(saved_path(&output).unwrap()).unwrap()).unwrap();
        assert_eq!(
            saved["content"],
            serde_json::to_value(&original.content).unwrap()
        );
        assert!(output.value["preview"].as_str().unwrap().len() < 500);
        assert!(!serde_json::to_string(&output).unwrap().contains("base64"));
    }
}
