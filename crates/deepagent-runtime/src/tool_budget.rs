//! Tool result budgeting and persistence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use deepagent_tools::ToolOutput;

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
    /// Whether to remove this run's tool result directory after the run ends.
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
            cleanup_on_run_end: true,
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

    /// Path for a single persisted tool result.
    pub fn result_path(&self, call_id: &str) -> PathBuf {
        self.output_dir
            .join(format!("{}.txt", sanitize_path_component(call_id)))
    }
}

/// Apply the configured budget to a tool output, persisting the original value
/// when the model-visible result would exceed the limit.
pub async fn apply_tool_result_budget(
    config: &ToolResultBudgetConfig,
    _session_id: &str,
    tool_name: &str,
    call_id: &str,
    mut output: ToolOutput,
) -> ToolOutput {
    if output.truncated {
        return output;
    }

    let serialized = match serde_json::to_string_pretty(&output.value) {
        Ok(s) => s,
        Err(_) => output.value.to_string(),
    };
    let estimated_tokens = estimate_tokens(&serialized);
    let max_tokens = config.limit_for(tool_name);
    if estimated_tokens <= max_tokens {
        return output;
    }

    let file_path = config.result_path(call_id);
    let saved_path = persist_full_output(&config.output_dir, &file_path, &serialized)
        .await
        .ok();
    let preview = truncate_chars(
        &serialized,
        config.preview_tokens.max(1).saturating_mul(CHARS_PER_TOKEN),
    );
    let saved_path_value = saved_path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "<not saved: write failed>".to_string());

    output.truncated = true;
    output.value = serde_json::json!({
        "truncated": true,
        "tool": tool_name,
        "call_id": call_id,
        "saved_path": saved_path_value,
        "original_estimated_tokens": estimated_tokens,
        "max_tokens": max_tokens,
        "preview": preview,
        "message": format!(
            "output truncated; full output saved to {}; use offset to read more when the tool supports it",
            saved_path_value
        ),
    });
    output
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

/// Remove files created by this run.
pub async fn cleanup_tool_result_paths(paths: Vec<PathBuf>) {
    for path in paths {
        let _ = tokio::fs::remove_file(path).await;
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
    out.push_str("\n... [output truncated; full output saved to disk]");
    out
}

async fn persist_full_output(
    dir: &Path,
    file_path: &Path,
    content: &str,
) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(file_path, content).await?;
    Ok(file_path.to_path_buf())
}

fn sanitize_path_component(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "result".to_string()
    } else {
        sanitized
    }
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
        assert!(saved.ends_with("call_1.txt"));
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
}
