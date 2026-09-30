//! Discovery + registration of untrusted WASM tools (D9).
//!
//! `SandboxedTool`/`WasmSandbox` in `deepagent-tools` were library-complete but
//! had no production instantiation: nothing anywhere built a wasm tool. This
//! module is that seam. It scans a workspace-local directory for compiled
//! `.wasm` tool modules and registers each as a [`SandboxedTool`] that runs in a
//! shared [`WasmSandbox`] with **no ambient authority** (deny-all capabilities,
//! fuel/memory/timeout caps), flowing through the SAME registry + permission
//! gate as native tools.
//!
//! Layout (mirrors the `.deepagent/agents` discovery convention):
//! ```text
//! <workspace>/.deepagent/wasm-tools/
//!   my_tool.wasm            # the untrusted guest module (JSON-in / JSON-out ABI)
//!   my_tool.json            # optional sidecar: { "description", "parameters" }
//! ```
//!
//! Compiled only with `--features wasm` (Wasmtime is heavyweight); the default
//! build omits it, matching `deepagent-tools`' own gating.

use std::path::Path;
use std::sync::Arc;

use deepagent_core::error::Result;
use deepagent_tools::sandbox::{Sandbox, SandboxPolicy};
use deepagent_tools::{SandboxedTool, ToolRegistry, WasmSandbox};

/// Workspace-relative directory scanned for wasm tool modules.
const WASM_TOOLS_SUBDIR: &str = ".deepagent/wasm-tools";

/// Optional sidecar descriptor for a wasm tool (`<name>.json`).
#[derive(Debug, serde::Deserialize)]
struct WasmToolSpec {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<serde_json::Value>,
}

/// Discover `<root>/.deepagent/wasm-tools/<name>.wasm` (plus an optional
/// `<name>.json` sidecar) and register each as a [`SandboxedTool`] sharing one
/// [`WasmSandbox`]. Returns the number of tools registered.
///
/// Best-effort per tool: an unreadable module or a sandbox-init failure is
/// logged and skipped rather than aborting the whole registry build.
pub(crate) fn register_wasm_tools(registry: &mut ToolRegistry, root: &Path) -> Result<usize> {
    let dir = root.join(WASM_TOOLS_SUBDIR);
    if !dir.is_dir() {
        return Ok(0);
    }
    let sandbox: Arc<dyn Sandbox> = match WasmSandbox::new() {
        Ok(sandbox) => Arc::new(sandbox),
        Err(error) => {
            tracing::warn!(%error, "WASM sandbox init failed; skipping wasm tools");
            return Ok(0);
        }
    };

    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%error, dir = %dir.display(), "failed to read wasm-tools dir");
            return Ok(0);
        }
    };

    let mut count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let module = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "failed to read wasm tool module");
                continue;
            }
        };
        let (description, parameters) =
            load_spec(&dir.join(format!("{name}.json"))).unwrap_or_else(|| default_spec(name));
        registry.register(Arc::new(SandboxedTool::new(
            name.to_string(),
            description,
            parameters,
            module,
            sandbox.clone(),
            SandboxPolicy::default(),
        )))?;
        count += 1;
    }
    if count > 0 {
        tracing::info!(count, "registered sandboxed wasm tools");
    }
    Ok(count)
}

/// Load the optional sidecar descriptor, falling back to defaults per field.
fn load_spec(path: &Path) -> Option<(String, serde_json::Value)> {
    let text = std::fs::read_to_string(path).ok()?;
    let spec: WasmToolSpec = serde_json::from_str(&text).ok()?;
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("wasm_tool");
    let (default_description, default_parameters) = default_spec(name);
    Some((
        spec.description.unwrap_or(default_description),
        spec.parameters.unwrap_or(default_parameters),
    ))
}

fn default_spec(name: &str) -> (String, serde_json::Value) {
    (
        format!("Sandboxed WASM tool '{name}' (runs untrusted code with no host authority)."),
        serde_json::json!({ "type": "object" }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_tools::permission::{Permission, PermissionSet};

    /// The echo guest from `deepagent-tools`' end-to-end test: `run` returns the
    /// input pointer/length unchanged, so the tool echoes its JSON input.
    const ECHO_WAT: &str = r#"
        (module
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 1024))
          (func (export "alloc") (param $len i32) (result i32)
            (local $p i32)
            global.get $bump
            local.set $p
            global.get $bump
            local.get $len
            i32.add
            global.set $bump
            local.get $p)
          (func (export "run") (param $ptr i32) (param $len i32) (result i64)
            local.get $ptr
            i64.extend_i32_u
            i64.const 32
            i64.shl
            local.get $len
            i64.extend_i32_u
            i64.or))
    "#;

    #[tokio::test]
    async fn discovers_and_runs_a_wasm_tool_through_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let tools_dir = root.join(WASM_TOOLS_SUBDIR);
        std::fs::create_dir_all(&tools_dir).unwrap();
        std::fs::write(
            tools_dir.join("echo.wasm"),
            wat::parse_str(ECHO_WAT).unwrap(),
        )
        .unwrap();
        std::fs::write(
            tools_dir.join("echo.json"),
            r#"{"description":"echoes JSON input","parameters":{"type":"object"}}"#,
        )
        .unwrap();

        let mut registry = ToolRegistry::new();
        let count = register_wasm_tools(&mut registry, root).unwrap();
        assert_eq!(count, 1, "one wasm tool should be discovered");

        // Flows through the same permission gate as a native tool.
        let granted = PermissionSet::from_iter_perms([Permission::Sandbox]);
        let out = registry
            .invoke(
                "echo",
                serde_json::json!({"hello": "wasm"}),
                &granted,
                false,
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["hello"], "wasm");

        // Without the Sandbox permission the registry denies it.
        assert!(registry
            .check("echo", &PermissionSet::read_only(), false)
            .is_err());
    }

    #[test]
    fn no_dir_registers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        assert_eq!(register_wasm_tools(&mut registry, dir.path()).unwrap(), 0);
    }
}
