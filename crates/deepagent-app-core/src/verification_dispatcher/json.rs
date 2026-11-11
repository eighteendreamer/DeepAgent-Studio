//! Syntax follows an identified consumer, never the presence of compilerOptions.
//! Local associations: tsconfig[.*].json / jsconfig[.*].json and their relative
//! extends/references, .jsonc, .code-workspace, and known .vscode configuration.
//! Unassociated JSON remains strict. No package resolution, schema downloads,
//! or file rewriting happens here. TypeScript configs delegate to the existing
//! compiler executor; JSONC parsing is not used as a TypeScript validator.

use super::{CommandExecutor, Path, TypeScriptVerifier, VerificationOutcome, Verifier};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Json,
    StrictManifest,
    Jsonc,
    TypeScript,
}

fn typescript_name(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    ["tsconfig", "jsconfig"].iter().any(|prefix| {
        name == format!("{prefix}.json")
            || name
                .strip_prefix(&format!("{prefix}."))
                .and_then(|s| s.strip_suffix(".json"))
                .is_some_and(|s| !s.is_empty())
    })
}

fn named_role(path: &Path) -> Role {
    if path.file_name().and_then(|s| s.to_str()) == Some("package.json") {
        // A known strict consumer takes precedence even if a TS config also
        // references it. Do not relax npm's manifest dialect.
        Role::StrictManifest
    } else if typescript_name(path) {
        Role::TypeScript
    } else if matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("jsonc" | "code-workspace")
    ) || (path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        == Some(".vscode")
        && matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some("settings.json" | "tasks.json" | "launch.json" | "extensions.json")
        ))
    {
        Role::Jsonc
    } else {
        Role::Json
    }
}

fn parse_config(content: &str) -> Result<serde_json::Value, String> {
    // TypeScript / VS Code accept a leading BOM. Do not strip it from data JSON.
    serde_json_lenient::from_str(content.strip_prefix('\u{feff}').unwrap_or(content))
        .map_err(|error| error.to_string())
}

/// Existing JSON verifier, now with explicit local consumer associations.
#[derive(Debug, Clone)]
pub struct JsonVerifier {
    typescript: TypeScriptVerifier,
}

impl JsonVerifier {
    /// Reuse the caller's existing command/permission/cancellation boundary.
    pub fn new(executor: Arc<dyn CommandExecutor>) -> Self {
        Self {
            typescript: TypeScriptVerifier::new(executor),
        }
    }
}

impl Default for JsonVerifier {
    fn default() -> Self {
        Self::new(Arc::new(deepagent_builtins::SystemExecutor))
    }
}

#[async_trait]
impl Verifier for JsonVerifier {
    fn name(&self) -> &str {
        "json"
    }

    fn handles(&self, path: &Path) -> bool {
        matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("json" | "jsonc" | "code-workspace")
        )
    }

    async fn verify(&self, path: &Path) -> VerificationOutcome {
        self.verify_in_workspace(path, None).await
    }

    async fn verify_in_workspace(&self, path: &Path, root: Option<&Path>) -> VerificationOutcome {
        // Resolve one host-native context before role discovery or changing
        // compiler cwd. Otherwise a relative project path is applied twice.
        // absolute() (Rust 1.79) does not access the filesystem or add the
        // canonicalize-only Windows verbatim prefix to ordinary DOS paths.
        let absolute_path = match std::path::absolute(path) {
            Ok(path) => path,
            Err(error) => return failed(format!("resolve verification path: {error}")),
        };
        let absolute_root = match root.map(std::path::absolute).transpose() {
            Ok(root) => root,
            Err(error) => return failed(format!("resolve verification workspace: {error}")),
        };
        let path = absolute_path.as_path();
        let root = absolute_root.as_deref();
        let content = match tokio::fs::read_to_string(path).await {
            Ok(content) => content,
            Err(error) => return failed(format!("read failed: {error}")),
        };
        let mut role = named_role(path);
        let strict = serde_json::from_str::<serde_json::Value>(&content);
        if role == Role::Json {
            match referenced_config(path, root).await {
                Reference::Found => role = Role::TypeScript,
                Reference::Unknown if strict.is_err() => return VerificationOutcome::Skipped {
                    reason: "configuration verification incomplete: local TypeScript role metadata could not be read; file dialect is unknown (未完成验证)".into(),
                },
                _ => {}
            }
        }
        if role == Role::TypeScript {
            return self.typescript.verify_config(path, root).await;
        }
        let result = if matches!(role, Role::Json | Role::StrictManifest) {
            strict.map_err(|error| error.to_string())
        } else {
            parse_config(&content)
        };
        match result {
            Ok(_) => VerificationOutcome::Passed,
            Err(error) => failed(format!("JSON parse error: {error}")),
        }
    }
}

fn failed(detail: String) -> VerificationOutcome {
    VerificationOutcome::Failed {
        detail,
        truncated: false,
    }
}

/// Discover from named configs alongside the file and in its ancestors, stopping
/// at the existing workspace. Follow only relative local edges, once per identity;
/// no recursive workspace scan, node_modules search, or out-of-workspace reads.
enum Reference {
    Found,
    Absent,
    Unknown,
}

async fn referenced_config(path: &Path, root: Option<&Path>) -> Reference {
    let Ok(target) = tokio::fs::canonicalize(path).await else {
        return Reference::Unknown;
    };
    let Some(root) = root.or_else(|| path.parent()) else {
        return Reference::Absent;
    };
    let Ok(root) = tokio::fs::canonicalize(root).await else {
        return Reference::Unknown;
    };
    if !target.starts_with(&root) {
        return Reference::Absent;
    }
    let guard = deepagent_builtins::WorkspaceRoot::new(&root);
    let mut pending = Vec::new();
    let mut unknown = false;
    let mut directory = target.parent();
    while let Some(dir) = directory.filter(|dir| dir.starts_with(&root)) {
        match tokio::fs::read_dir(dir).await {
            Ok(mut entries) => loop {
                match entries.next_entry().await {
                    Ok(Some(entry)) if typescript_name(&entry.path()) => pending.push(entry.path()),
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => {
                        unknown = true;
                        break;
                    }
                }
            },
            Err(_) => unknown = true,
        }
        directory = dir.parent();
    }
    let mut visited = HashSet::new();
    while let Some(candidate) = pending.pop() {
        // Canonicalization is identity discovery only. The existing filesystem
        // guard still authorizes the resolved path before reading its contents.
        let Ok(candidate) = tokio::fs::canonicalize(candidate).await else {
            continue;
        };
        if !candidate.starts_with(&root) || !visited.insert(candidate.clone()) {
            continue;
        }
        if guard.resolve_read(&candidate.to_string_lossy()).is_err() {
            continue;
        }
        if candidate == target {
            return Reference::Found;
        }
        let Ok(text) = tokio::fs::read_to_string(&candidate).await else {
            unknown = true;
            continue;
        };
        let Ok(config) = parse_config(&text) else {
            unknown = true;
            continue;
        };
        let dir = candidate.parent().unwrap_or(&root);
        let mut edges = Vec::new();
        match &config["extends"] {
            serde_json::Value::String(edge) => edges.push((edge.as_str(), false)),
            serde_json::Value::Array(edges_array) => edges.extend(
                edges_array
                    .iter()
                    .filter_map(|edge| edge.as_str().map(|s| (s, false))),
            ),
            _ => {}
        }
        if let Some(references) = config["references"].as_array() {
            edges.extend(
                references
                    .iter()
                    .filter_map(|edge| edge["path"].as_str().map(|s| (s, true))),
            );
        }
        for (edge, reference) in edges {
            let edge = edge.replace('\\', "/");
            // TypeScript references are relative paths; extends also permits
            // package specifiers, deliberately not resolved by this syntax check.
            if !(reference || edge.starts_with("./") || edge.starts_with("../")) {
                continue;
            }
            if Path::new(&edge).is_absolute() || edge.contains(':') || edge.starts_with(['/', '\\'])
            {
                continue;
            }
            // TypeScript normalizes relative configuration paths lexically.
            // Keep that normalization inside the already canonical workspace,
            // then reuse the existing physical/sensitive-path guard.
            let mut next = dir.to_path_buf();
            let mut confined = true;
            for component in Path::new(&edge).components() {
                match component {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir if next != root => {
                        next.pop();
                    }
                    std::path::Component::Normal(name) => next.push(name),
                    _ => {
                        confined = false;
                        break;
                    }
                }
            }
            if !confined || guard.resolve_read(&next.to_string_lossy()).is_err() {
                continue;
            }
            if reference && next.extension().and_then(|value| value.to_str()) != Some("json") {
                next = next.join("tsconfig.json");
            } else if !reference
                && !next.is_file()
                && next.extension().and_then(|value| value.to_str()) != Some("json")
            {
                next.as_mut_os_string().push(".json");
            }
            pending.push(next);
        }
    }
    if unknown {
        Reference::Unknown
    } else {
        Reference::Absent
    }
}
