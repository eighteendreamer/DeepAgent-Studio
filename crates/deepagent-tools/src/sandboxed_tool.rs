//! [`SandboxedTool`] — run an untrusted tool as a sandboxed WASM module.
//!
//! This is the bridge between the [`Sandbox`] backend and the [`Tool`] trait:
//! it wraps a compiled WASM module so an untrusted/third-party tool flows
//! through the **same capability registry, permission gating, and runtime loop**
//! as a native built-in — the model and the runtime cannot tell the difference.
//!
//! Unlike a native [`Tool`] (trusted Rust code), a `SandboxedTool` executes
//! guest bytecode with **no ambient authority**: a [`SandboxPolicy`] caps memory,
//! CPU (fuel), wall-clock time, and output size, and the module is instantiated
//! with no host imports (see [`crate::sandbox`] / [`crate::wasm`]). This is the
//! piece Claude Code / Codex lack — their built-in tools are trusted and only
//! `Bash`/shell is OS-sandboxed; here *any* tool can be confined.
//!
//! The synchronous, CPU-bound sandbox run is dispatched onto a blocking task so
//! it never stalls the async runtime.
//!
//! ## ABI
//!
//! The wrapped module receives the invocation **arguments JSON** as input and
//! returns a JSON value as output (the guest ABI in [`crate::sandbox`]). The
//! returned JSON becomes the [`ToolOutput`] value; a guest that returns
//! `{ "error": "..." }` (or non-JSON / a trap) is surfaced as a failed
//! [`ToolOutput`] rather than aborting the run.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::Result;

use crate::sandbox::{Sandbox, SandboxPolicy};
use crate::{Permission, PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolOutput};

/// A tool whose logic is an untrusted WASM module executed in a [`Sandbox`].
pub struct SandboxedTool {
    descriptor: ToolDescriptor,
    /// The compiled WASM module bytes.
    module: Arc<Vec<u8>>,
    /// The sandbox backend (shared; e.g. a `WasmSandbox`).
    sandbox: Arc<dyn Sandbox>,
    /// Resource + capability policy applied to every invocation.
    policy: SandboxPolicy,
}

impl SandboxedTool {
    /// Build a sandboxed tool.
    ///
    /// `name`/`description`/`parameters` form the descriptor advertised to the
    /// model. Sandboxed tools are classified [`RiskLevel::Medium`] by default
    /// (they run untrusted code, but inside a no-authority sandbox) and require
    /// no host permissions unless the caller overrides via
    /// [`SandboxedTool::with_descriptor`].
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
        module: impl Into<Vec<u8>>,
        sandbox: Arc<dyn Sandbox>,
        policy: SandboxPolicy,
    ) -> Self {
        let descriptor = ToolDescriptor {
            name: name.into(),
            description: description.into(),
            parameters,
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([Permission::Sandbox]),
        };
        Self {
            descriptor,
            module: Arc::new(module.into()),
            sandbox,
            policy,
        }
    }

    /// Override the full descriptor (e.g. to set a different risk/permission).
    pub fn with_descriptor(mut self, descriptor: ToolDescriptor) -> Self {
        self.descriptor = descriptor;
        self
    }

    /// The policy applied to invocations.
    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }
}

#[async_trait]
impl Tool for SandboxedTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        let input = arguments.to_string();
        let module = self.module.clone();
        let sandbox = self.sandbox.clone();
        let policy = self.policy;

        // The sandbox run is synchronous + CPU-bound: never block the async
        // runtime — dispatch it to a blocking thread.
        let result = tokio::task::spawn_blocking(move || {
            sandbox.run_json(module.as_slice(), &input, &policy)
        })
        .await;

        let run = match result {
            Ok(r) => r,
            Err(join_err) => {
                return Ok(ToolOutput::failure(format!(
                    "sandbox task failed to join: {join_err}"
                )));
            }
        };

        match run {
            Ok((output_json, _stats)) => {
                // Parse the guest's JSON; map a guest-reported error to failure.
                match serde_json::from_str::<serde_json::Value>(&output_json) {
                    Ok(value) => {
                        let is_error = value.get("error").map(|e| !e.is_null()).unwrap_or(false);
                        Ok(ToolOutput {
                            ok: !is_error,
                            value,
                            truncated: false,
                        })
                    }
                    Err(e) => Ok(ToolOutput::failure(format!(
                        "sandboxed tool returned non-JSON output: {e}"
                    ))),
                }
            }
            // A policy violation / trap (fuel, timeout, memory, bad module)
            // becomes a failed observation the agent can react to.
            Err(e) => Ok(ToolOutput::failure(format!("sandbox error: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::SandboxStats;
    use crate::ToolRegistry;
    use deepagent_core::error::CoreError;

    /// A mock sandbox: echoes the input back as the output (proving the ABI
    /// plumbing without needing the wasm engine).
    struct EchoSandbox;
    impl Sandbox for EchoSandbox {
        fn run_json(
            &self,
            _wasm: &[u8],
            input_json: &str,
            _policy: &SandboxPolicy,
        ) -> Result<(String, SandboxStats)> {
            Ok((
                input_json.to_string(),
                SandboxStats {
                    fuel_used: 42,
                    output_bytes: input_json.len(),
                },
            ))
        }
    }

    /// A mock sandbox that always traps (policy violation).
    struct TrapSandbox;
    impl Sandbox for TrapSandbox {
        fn run_json(
            &self,
            _wasm: &[u8],
            _input_json: &str,
            _policy: &SandboxPolicy,
        ) -> Result<(String, SandboxStats)> {
            Err(CoreError::other("sandbox CPU (fuel) budget exhausted"))
        }
    }

    /// A mock sandbox returning a guest-reported error object.
    struct GuestErrorSandbox;
    impl Sandbox for GuestErrorSandbox {
        fn run_json(
            &self,
            _wasm: &[u8],
            _input_json: &str,
            _policy: &SandboxPolicy,
        ) -> Result<(String, SandboxStats)> {
            Ok((
                r#"{"error":"bad input"}"#.to_string(),
                SandboxStats::default(),
            ))
        }
    }

    fn tool(sandbox: Arc<dyn Sandbox>) -> SandboxedTool {
        SandboxedTool::new(
            "echo_wasm",
            "echoes its JSON input",
            serde_json::json!({"type": "object"}),
            b"\0asm".to_vec(),
            sandbox,
            SandboxPolicy::default(),
        )
    }

    #[test]
    fn descriptor_is_medium_risk_and_needs_sandbox_perm() {
        let t = tool(Arc::new(EchoSandbox));
        let d = t.descriptor();
        assert_eq!(d.name, "echo_wasm");
        assert_eq!(d.risk, RiskLevel::Medium);
        assert!(d.required_permissions.contains(Permission::Sandbox));
    }

    #[tokio::test]
    async fn invoke_roundtrips_json() {
        let t = tool(Arc::new(EchoSandbox));
        let out = t
            .invoke(serde_json::json!({"hello": "world"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["hello"], "world");
    }

    #[tokio::test]
    async fn trap_becomes_failed_output() {
        let t = tool(Arc::new(TrapSandbox));
        let out = t.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
        assert!(out.value["error"]
            .as_str()
            .unwrap()
            .contains("sandbox error"));
    }

    #[tokio::test]
    async fn guest_reported_error_is_failure() {
        let t = tool(Arc::new(GuestErrorSandbox));
        let out = t.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
        assert_eq!(out.value["error"], "bad input");
    }

    #[tokio::test]
    async fn runs_through_registry_with_permission() {
        // A SandboxedTool flows through the same registry + permission gate as a
        // native tool.
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(tool(Arc::new(EchoSandbox))))
            .unwrap();

        let granted = PermissionSet::from_iter_perms([Permission::Sandbox]);
        let out = registry
            .invoke("echo_wasm", serde_json::json!({"x": 1}), &granted, false)
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["x"], 1);

        // Without the Sandbox permission, the registry denies it.
        let denied = registry.check("echo_wasm", &PermissionSet::read_only(), false);
        assert!(denied.is_err());
    }

    /// End-to-end with the real Wasmtime backend: a hand-written echo module
    /// wrapped as a SandboxedTool roundtrips JSON through actual WASM execution.
    #[cfg(feature = "wasm")]
    #[tokio::test]
    async fn end_to_end_with_real_wasm_backend() {
        use crate::wasm::WasmSandbox;

        // Echo guest: alloc bump-allocates, run echoes (out_ptr=in_ptr, out_len=in_len).
        let wat = r#"
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
        let module = wat::parse_str(wat).unwrap();
        let sandbox: Arc<dyn Sandbox> = Arc::new(WasmSandbox::new().unwrap());
        let tool = SandboxedTool::new(
            "echo_wasm",
            "echo",
            serde_json::json!({"type": "object"}),
            module,
            sandbox,
            SandboxPolicy::default(),
        );
        let out = tool
            .invoke(serde_json::json!({"msg": "hi from wasm"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["msg"], "hi from wasm");
    }
}
