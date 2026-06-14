//! Tool-result decorator extension point.
//!
//! Phases 3 and 4 of the coding-amplifier spec inject metadata into individual
//! tool results — Plan-mode reminders (Phase 3B), todo-list snapshots
//! (Phase 3E), post-edit verification status (Phase 4B), etc. The runtime owns
//! the tool-result emission path but doesn't know about session-level concepts
//! like "plan mode" or "todo store"; those live in higher crates such as
//! `deepagent-app-core`.
//!
//! Rather than coupling the runtime to those concepts, this trait lets a
//! higher-level crate hand the runtime a single decorator that mutates each
//! [`ToolOutput`] in place after invocation. Implementations typically call
//! [`crate::empty_stub::ensure_non_empty_output`]'s sister helpers (e.g. the
//! `system_reminder::append_to_tool_result` in `deepagent-app-core`) to attach
//! `<system-reminder>` envelopes.
//!
//! Decorators run **after** the budget step and the empty-output stub. They
//! never change `ok` (verification's strict mode flips `ok` separately at the
//! call site, not via this trait).

use std::fmt::Debug;
use std::sync::Arc;

use async_trait::async_trait;

use deepagent_tools::ToolOutput;

/// Mutates a tool result in place to attach run-time metadata.
///
/// `Send + Sync + Debug` is required so a decorator can live behind an `Arc`
/// inside [`crate::loop_engine::RuntimeConfig`], which is itself `Clone +
/// Debug`. The trait is intentionally tiny; callers should compose multiple
/// decorators by wrapping them in a [`ChainDecorator`] or by writing their own
/// aggregator that holds the constituent state.
///
/// `decorate` is async because Phase 4 verifiers spawn subprocesses
/// (cargo / tsc / python) that the runtime needs to await without blocking
/// the async executor.
#[async_trait]
pub trait ToolResultDecorator: Debug + Send + Sync {
    /// Inspect / mutate `output` for the call to `tool_name`. Implementations
    /// must not change `output.ok`.
    async fn decorate(&self, tool_name: &str, output: &mut ToolOutput);
}

/// Compose several decorators in order. Each is applied to the same output;
/// later decorators see the mutations made by earlier ones.
#[derive(Debug, Clone, Default)]
pub struct ChainDecorator {
    decorators: Vec<Arc<dyn ToolResultDecorator>>,
}

impl ChainDecorator {
    /// Build an empty chain.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a decorator to the end of the chain.
    pub fn push(mut self, decorator: Arc<dyn ToolResultDecorator>) -> Self {
        self.decorators.push(decorator);
        self
    }

    /// Number of registered decorators.
    pub fn len(&self) -> usize {
        self.decorators.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.decorators.is_empty()
    }
}

#[async_trait]
impl ToolResultDecorator for ChainDecorator {
    async fn decorate(&self, tool_name: &str, output: &mut ToolOutput) {
        for d in &self.decorators {
            d.decorate(tool_name, output).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct CountingDecorator {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ToolResultDecorator for CountingDecorator {
        async fn decorate(&self, tool_name: &str, output: &mut ToolOutput) {
            self.calls.lock().unwrap().push(tool_name.to_string());
            if let Value::Object(map) = &mut output.value {
                map.insert("decorated_by".into(), Value::String("counting".into()));
            }
        }
    }

    #[derive(Debug)]
    struct AppendingDecorator(&'static str);
    #[async_trait]
    impl ToolResultDecorator for AppendingDecorator {
        async fn decorate(&self, _: &str, output: &mut ToolOutput) {
            if let Value::Object(map) = &mut output.value {
                map.insert(format!("tag_{}", self.0), Value::Bool(true));
            }
        }
    }

    fn ok_with(value: Value) -> ToolOutput {
        ToolOutput {
            ok: true,
            value,
            truncated: false,
        }
    }

    #[tokio::test]
    async fn single_decorator_mutates_output() {
        let dec = CountingDecorator::default();
        let mut out = ok_with(json!({"x": 1}));
        dec.decorate("read_file", &mut out).await;
        assert_eq!(out.value["decorated_by"], "counting");
        assert_eq!(*dec.calls.lock().unwrap(), vec!["read_file".to_string()]);
    }

    #[tokio::test]
    async fn chain_runs_decorators_in_order() {
        let chain = ChainDecorator::new()
            .push(Arc::new(AppendingDecorator("a")))
            .push(Arc::new(AppendingDecorator("b")));
        let mut out = ok_with(json!({}));
        chain.decorate("bash", &mut out).await;
        assert_eq!(out.value["tag_a"], true);
        assert_eq!(out.value["tag_b"], true);
        assert_eq!(chain.len(), 2);
    }

    #[tokio::test]
    async fn empty_chain_is_noop() {
        let chain = ChainDecorator::new();
        assert!(chain.is_empty());
        let mut out = ok_with(json!({"x": 1}));
        chain.decorate("any", &mut out).await;
        assert_eq!(out.value, json!({"x": 1}));
    }

    #[tokio::test]
    async fn decorator_does_not_flip_ok_status() {
        let dec = CountingDecorator::default();
        let mut out = ok_with(json!({}));
        dec.decorate("t", &mut out).await;
        assert!(out.ok);

        let mut err = ToolOutput {
            ok: false,
            value: json!({}),
            truncated: false,
        };
        dec.decorate("t", &mut err).await;
        assert!(!err.ok);
    }
}
