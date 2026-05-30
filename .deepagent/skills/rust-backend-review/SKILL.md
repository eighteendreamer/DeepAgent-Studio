---
name: Rust Backend Review
description: This skill should be used when the user asks to "review rust code", "audit a rust crate", "check error handling", "review unsafe code", or wants Rust backend quality guidance (ownership, error types, async, clippy).
version: 0.1.0
---

# Rust Backend Review

Provide a focused, high-signal review of Rust backend code. Apply the checklist
below, cite concrete line references, and prefer fixes that match the crate's
existing conventions.

## Review checklist

1. **Error handling** — public APIs return `Result<_, E>` with a meaningful
   error type (no `unwrap`/`expect` on fallible paths outside tests). `?`
   propagation is used over manual matching where it reads cleanly.
2. **Ownership & borrowing** — avoid needless `clone()`; prefer borrows. Flag
   `Arc<Mutex<_>>` where a simpler ownership model would do.
3. **Async correctness** — no blocking calls inside async without
   `spawn_blocking`; no holding a `std::sync::Mutex` guard across `.await`.
4. **Safety** — every `unsafe` block has a `// SAFETY:` comment justifying the
   invariant. Prefer safe abstractions.
5. **API surface** — public items documented; `#[must_use]` on builders;
   minimal `pub` exposure.
6. **Tests** — new behavior has unit tests; error paths are exercised.

## Process

To review, do the following in order:

1. Identify the changed/target files and read them fully.
2. Walk the checklist top to bottom, noting each finding with `file:line`.
3. Group findings by severity (blocker / should-fix / nit).
4. Propose concrete diffs for blockers; describe should-fix and nits briefly.
5. Run `cargo clippy --all-targets -- -D warnings` and `cargo test` if available
   and fold the results into the review.

## Bundled resources

- `references/checklist.md` — the extended review checklist with rationale and
  examples for each item (load when deeper justification is needed).
