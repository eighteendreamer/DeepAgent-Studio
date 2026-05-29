# DeepAgent Studio

A DeepSeek-native **Agent Runtime Platform** — not just an AI IDE. The core is a
verifiable, replayable Runtime Kernel (context engineering, thinking-mode
persistence, sub-agent orchestration, verification, safety) with the IDE as one
UI on top.

> Roadmap: see [`开发计划.md`](./开发计划.md). Design philosophy: see
> [`开发提示词.md`](./开发提示词.md). Living architecture map:
> [`ARCHITECTURE.md`](./ARCHITECTURE.md).

## Status

**Phase 1 (Core Infrastructure) — complete**, plus the foundational slices of the
Phase 2–5 kernel crates. The whole workspace builds, is lint-clean, and is
covered by unit + end-to-end tests.

Implemented today:

- Cargo workspace with 8 kernel crates + a headless CLI driver.
- Append-only **event store** on SQLite with versioned migrations (the source
  of truth for the whole system).
- **Session manager** with replay + crash recovery folded purely from events.
- **Task state machine**, strongly-typed IDs, testable clock.
- **Prompt Compiler AST** + token-budget system.
- **Multi-tier memory** with importance/recency/decay ranking.
- **Capability registry** with a permission + risk model.
- The **Agent Runtime Loop** (THINK → EXECUTE → OBSERVE) driving a pluggable
  agent brain, with metrics.

## Prerequisites

- Rust (stable, 1.80+). The repo pins a toolchain via `rust-toolchain.toml`.
- SQLite is bundled (no system install needed).

## Quick start

```bash
# Run the full test suite
cargo test --workspace

# Run the end-to-end kernel demo: opens a DB, runs a scripted agent through a
# tool call, then recovers the session purely from the event log.
cargo run -p deepagent-cli
```

## Layout

```text
crates/
  deepagent-core         domain primitives (ids, events, tasks, messages, clock)
  deepagent-tracing      tracing + metrics
  deepagent-persistence  sqlite, migrations, append-only event store
  deepagent-session      session manager (replay + recovery)
  deepagent-context      prompt compiler AST + token budget
  deepagent-memory       multi-tier memory + ranking
  deepagent-tools        tool trait, capability registry, permissions
  deepagent-runtime      the agent runtime loop
apps/
  cli                    headless smoke-test driver
```

## License

Apache-2.0
