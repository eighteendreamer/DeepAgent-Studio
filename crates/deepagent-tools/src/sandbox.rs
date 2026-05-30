//! Tool sandboxing (开发提示词.md §15–§16; 开发计划.md Phase 4).
//!
//! Untrusted tool code can be executed inside a WebAssembly sandbox with **no
//! ambient authority** — no filesystem, network, clock, or environment unless a
//! capability is explicitly granted. This module defines the parts that are
//! independent of the WASM engine so they are testable offline:
//!
//! - [`SandboxPolicy`] — resource limits (memory, CPU "fuel", wall-clock
//!   timeout) and capability grants ([`Capabilities`]). Default denies all.
//! - [`SandboxStats`] — what a run consumed (fuel, output size).
//! - [`Sandbox`] — the trait a backend implements (the real one is the
//!   feature-gated Wasmtime backend in [`crate::wasm`]).
//! - ABI helpers ([`pack_ptr_len`] / [`unpack_ptr_len`]) for the JSON-in /
//!   JSON-out calling convention shared with guest modules.
//!
//! The Wasmtime backend ([`crate::wasm::WasmSandbox`], behind `--features wasm`)
//! enforces [`SandboxPolicy`] via engine fuel, store memory limits, and epoch
//! interruption. Without the feature, the policy/ABI logic still compiles and is
//! fully unit-tested.

use deepagent_core::error::{CoreError, Result};

/// Capability grants for a sandboxed run. Every capability defaults to `false`
/// (deny): a fresh sandbox has no access to the host at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// Allow read-only filesystem access (via WASI preopens, when wired).
    pub fs_read: bool,
    /// Allow filesystem writes.
    pub fs_write: bool,
    /// Allow outbound network access.
    pub network: bool,
    /// Allow reading the wall clock / random.
    pub clock: bool,
}

impl Capabilities {
    /// No capabilities (the safe default).
    pub fn none() -> Self {
        Self::default()
    }

    /// Whether any host capability is granted.
    pub fn grants_any(&self) -> bool {
        self.fs_read || self.fs_write || self.network || self.clock
    }
}

/// Resource + capability policy enforced on a sandboxed execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxPolicy {
    /// Hard cap on linear memory the guest may allocate, in bytes.
    pub max_memory_bytes: usize,
    /// CPU budget as Wasmtime "fuel" units (roughly one per executed bytecode
    /// op). `None` means unlimited (not recommended for untrusted code).
    pub fuel: Option<u64>,
    /// Wall-clock timeout in milliseconds (enforced via epoch interruption).
    pub timeout_ms: u64,
    /// Maximum size of the JSON output the guest may return, in bytes.
    pub max_output_bytes: usize,
    /// Host capabilities granted to the guest.
    pub capabilities: Capabilities,
}

impl Default for SandboxPolicy {
    /// A conservative default for untrusted tools: 64 MiB memory, 100M fuel,
    /// 5 s timeout, 1 MiB output cap, and **no** capabilities.
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024,
            fuel: Some(100_000_000),
            timeout_ms: 5_000,
            max_output_bytes: 1024 * 1024,
            capabilities: Capabilities::none(),
        }
    }
}

impl SandboxPolicy {
    /// A stricter policy for fully untrusted code: 16 MiB, 10M fuel, 1 s.
    pub fn untrusted() -> Self {
        Self {
            max_memory_bytes: 16 * 1024 * 1024,
            fuel: Some(10_000_000),
            timeout_ms: 1_000,
            max_output_bytes: 256 * 1024,
            capabilities: Capabilities::none(),
        }
    }

    /// Validate the policy is internally consistent.
    pub fn validate(&self) -> Result<()> {
        if self.max_memory_bytes == 0 {
            return Err(CoreError::invalid("sandbox max_memory_bytes must be > 0"));
        }
        if self.timeout_ms == 0 {
            return Err(CoreError::invalid("sandbox timeout_ms must be > 0"));
        }
        if self.max_output_bytes == 0 {
            return Err(CoreError::invalid("sandbox max_output_bytes must be > 0"));
        }
        if matches!(self.fuel, Some(0)) {
            return Err(CoreError::invalid(
                "sandbox fuel must be > 0 (or None for unlimited)",
            ));
        }
        Ok(())
    }

    /// Number of Wasmtime memory pages (64 KiB each) the memory cap implies.
    pub fn memory_pages(&self) -> usize {
        const PAGE: usize = 64 * 1024;
        self.max_memory_bytes.div_ceil(PAGE)
    }
}

/// What a sandboxed run consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SandboxStats {
    /// Fuel consumed (if fuel metering was enabled).
    pub fuel_used: u64,
    /// Size of the returned output in bytes.
    pub output_bytes: usize,
}

/// A sandbox that executes a WASM module with a JSON input and returns JSON.
///
/// Implementors enforce the [`SandboxPolicy`] (memory, fuel, timeout). The guest
/// module follows the ABI in [`pack_ptr_len`]. The trait is `Send + Sync` so a
/// sandbox can be shared (`Arc<dyn Sandbox>`) and moved onto a blocking task.
pub trait Sandbox: Send + Sync {
    /// Execute `wasm` (a compiled module's bytes) against `input_json`, applying
    /// `policy`, returning the guest's JSON output plus run stats.
    fn run_json(
        &self,
        wasm: &[u8],
        input_json: &str,
        policy: &SandboxPolicy,
    ) -> Result<(String, SandboxStats)>;
}

/// Pack a `(ptr, len)` pair into the `i64` the guest's entry point returns:
/// `(ptr as u32) << 32 | (len as u32)`.
pub fn pack_ptr_len(ptr: u32, len: u32) -> i64 {
    (((ptr as u64) << 32) | (len as u64)) as i64
}

/// Unpack an `i64` returned by the guest into `(ptr, len)`.
pub fn unpack_ptr_len(packed: i64) -> (u32, u32) {
    let bits = packed as u64;
    ((bits >> 32) as u32, (bits & 0xFFFF_FFFF) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_denies_all_capabilities() {
        let p = SandboxPolicy::default();
        assert!(!p.capabilities.grants_any());
        assert!(p.validate().is_ok());
        assert!(p.fuel.unwrap() > 0);
    }

    #[test]
    fn untrusted_is_stricter() {
        let u = SandboxPolicy::untrusted();
        let d = SandboxPolicy::default();
        assert!(u.max_memory_bytes < d.max_memory_bytes);
        assert!(u.fuel.unwrap() < d.fuel.unwrap());
        assert!(u.timeout_ms < d.timeout_ms);
    }

    #[test]
    fn validate_rejects_zero_limits() {
        let bad = SandboxPolicy {
            max_memory_bytes: 0,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        let bad_fuel = SandboxPolicy {
            fuel: Some(0),
            ..Default::default()
        };
        assert!(bad_fuel.validate().is_err());
    }

    #[test]
    fn memory_pages_rounds_up() {
        let p = SandboxPolicy {
            max_memory_bytes: 64 * 1024 + 1,
            ..Default::default()
        };
        assert_eq!(p.memory_pages(), 2);
        let exact = SandboxPolicy {
            max_memory_bytes: 64 * 1024,
            ..Default::default()
        };
        assert_eq!(exact.memory_pages(), 1);
    }

    #[test]
    fn ptr_len_packing_roundtrip() {
        let cases = [
            (0u32, 0u32),
            (1, 2),
            (0x1234_5678, 0x9ABC_DEF0),
            (u32::MAX, u32::MAX),
        ];
        for (ptr, len) in cases {
            let packed = pack_ptr_len(ptr, len);
            assert_eq!(unpack_ptr_len(packed), (ptr, len));
        }
    }

    #[test]
    fn capabilities_grant_detection() {
        assert!(!Capabilities::none().grants_any());
        assert!(Capabilities {
            network: true,
            ..Default::default()
        }
        .grants_any());
    }
}
