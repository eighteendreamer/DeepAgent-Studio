//! The self-healing loop (开发计划.md Phase 7 — "Self-Healing Agent").
//!
//! Ties the [`Verifier`] and [`ReflectionEngine`] together with a pluggable
//! [`Fixer`] to implement:
//!
//! ```text
//! VERIFY -> (pass? done) -> REFLECT -> FIX -> RETRY -> VERIFY -> ...
//! ```
//!
//! The loop terminates when verification passes, reflection recommends giving
//! up (including loop detection), or the attempt budget is exhausted.

use async_trait::async_trait;

use deepagent_core::error::Result;

use crate::reflection::{NextAction, Reflection, ReflectionEngine};
use crate::runner::CommandRunner;
use crate::verifier::{VerificationStep, Verifier};

/// Attempts to fix the problem described by a [`Reflection`]. In production this
/// is backed by the agent/model; here it is a trait so the loop is testable.
#[async_trait]
pub trait Fixer: Send {
    /// Attempt a fix given the latest reflection. Returns `Ok(true)` if a fix
    /// was applied (worth retrying), `Ok(false)` if no fix could be made.
    async fn attempt_fix(&mut self, reflection: &Reflection) -> Result<bool>;
}

/// The final outcome of a self-healing run.
#[derive(Debug, Clone, PartialEq)]
pub enum HealingOutcome {
    /// Verification ultimately passed.
    Healed {
        /// How many attempts it took (1 = passed first try).
        attempts: u32,
    },
    /// Gave up (loop detected, attempt cap, or fixer could not fix).
    Failed {
        /// The final reflection explaining why.
        reflection: Reflection,
    },
}

impl HealingOutcome {
    /// Whether the run ended healed.
    pub fn is_healed(&self) -> bool {
        matches!(self, HealingOutcome::Healed { .. })
    }
}

/// Runs the verify → reflect → fix → retry loop.
pub struct SelfHealingLoop<R: CommandRunner> {
    verifier: Verifier<R>,
    reflection: ReflectionEngine,
}

impl<R: CommandRunner> SelfHealingLoop<R> {
    /// Build the loop from a verifier and a reflection engine.
    pub fn new(verifier: Verifier<R>, reflection: ReflectionEngine) -> Self {
        Self {
            verifier,
            reflection,
        }
    }

    /// Run the loop over `steps`, driving `fixer` between attempts.
    pub async fn run(
        &mut self,
        steps: &[VerificationStep],
        fixer: &mut dyn Fixer,
    ) -> Result<HealingOutcome> {
        loop {
            let report = self.verifier.run_suite(steps).await?;
            let reflection = self.reflection.reflect(&report);

            match reflection.action {
                NextAction::Proceed => {
                    return Ok(HealingOutcome::Healed {
                        attempts: reflection.attempt,
                    });
                }
                NextAction::GiveUp => {
                    return Ok(HealingOutcome::Failed { reflection });
                }
                NextAction::Retry => {
                    tracing::info!(
                        attempt = reflection.attempt,
                        kind = ?reflection.failure_kind,
                        "verification failed; attempting fix"
                    );
                    let fixed = fixer.attempt_fix(&reflection).await?;
                    if !fixed {
                        // The fixer could not address it: stop rather than spin.
                        return Ok(HealingOutcome::Failed { reflection });
                    }
                    // Loop back to re-verify.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflection::ReflectionEngine;
    use crate::runner::{Command, CommandOutput, CommandRunner, MockRunner};
    use crate::verifier::Verifier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn build_step() -> Vec<VerificationStep> {
        vec![VerificationStep::build(Command::parse("cargo build"))]
    }

    /// A runner that fails for the first `fail_times` calls, then succeeds.
    struct FlakyRunner {
        calls: AtomicUsize,
        fail_times: usize,
    }

    #[async_trait]
    impl CommandRunner for FlakyRunner {
        async fn run(&self, _command: &Command) -> Result<CommandOutput> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_times {
                Ok(CommandOutput {
                    exit_code: Some(1),
                    stdout: String::new(),
                    stderr: "error[E0001]: boom".into(),
                })
            } else {
                Ok(CommandOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
    }

    struct CountingFixer {
        fixes: Arc<AtomicUsize>,
        can_fix: bool,
    }

    #[async_trait]
    impl Fixer for CountingFixer {
        async fn attempt_fix(&mut self, _reflection: &Reflection) -> Result<bool> {
            self.fixes.fetch_add(1, Ordering::SeqCst);
            Ok(self.can_fix)
        }
    }

    #[tokio::test]
    async fn heals_after_fixes() {
        // Fails twice, then passes.
        let runner = FlakyRunner {
            calls: AtomicUsize::new(0),
            fail_times: 2,
        };
        let fixes = Arc::new(AtomicUsize::new(0));
        let mut loop_ = SelfHealingLoop::new(Verifier::new(runner), ReflectionEngine::new(5, 10));
        let mut fixer = CountingFixer {
            fixes: fixes.clone(),
            can_fix: true,
        };
        let outcome = loop_.run(&build_step(), &mut fixer).await.unwrap();
        assert!(outcome.is_healed());
        if let HealingOutcome::Healed { attempts } = outcome {
            assert_eq!(attempts, 3); // fail, fail, pass
        }
        assert_eq!(fixes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn passes_first_try() {
        let runner = MockRunner::new().with_success("cargo build");
        let fixes = Arc::new(AtomicUsize::new(0));
        let mut loop_ =
            SelfHealingLoop::new(Verifier::new(runner), ReflectionEngine::with_defaults());
        let mut fixer = CountingFixer {
            fixes: fixes.clone(),
            can_fix: true,
        };
        let outcome = loop_.run(&build_step(), &mut fixer).await.unwrap();
        assert_eq!(outcome, HealingOutcome::Healed { attempts: 1 });
        assert_eq!(fixes.load(Ordering::SeqCst), 0); // never needed a fix
    }

    #[tokio::test]
    async fn gives_up_when_fixer_cannot_fix() {
        let runner = MockRunner::new().with_failure("cargo build", "error[E0001]: nope");
        let fixes = Arc::new(AtomicUsize::new(0));
        let mut loop_ =
            SelfHealingLoop::new(Verifier::new(runner), ReflectionEngine::with_defaults());
        let mut fixer = CountingFixer {
            fixes: fixes.clone(),
            can_fix: false,
        };
        let outcome = loop_.run(&build_step(), &mut fixer).await.unwrap();
        assert!(!outcome.is_healed());
        assert_eq!(fixes.load(Ordering::SeqCst), 1); // tried once, gave up
    }

    #[tokio::test]
    async fn detects_infinite_loop() {
        // Always fails with the same signature; fixer claims it fixed but never
        // does. Loop detection must stop it.
        let runner = MockRunner::new().with_failure("cargo build", "error[E0001]: same every time");
        let fixes = Arc::new(AtomicUsize::new(0));
        let mut loop_ = SelfHealingLoop::new(
            Verifier::new(runner),
            ReflectionEngine::new(2, 100), // repeat cap 2
        );
        let mut fixer = CountingFixer {
            fixes: fixes.clone(),
            can_fix: true,
        };
        let outcome = loop_.run(&build_step(), &mut fixer).await.unwrap();
        match outcome {
            HealingOutcome::Failed { reflection } => {
                assert!(reflection.diagnosis.contains("infinite retry loop"));
            }
            other => panic!("expected Failed due to loop detection, got {other:?}"),
        }
    }
}
