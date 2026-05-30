# Extended Rust Backend Review Checklist

This reference expands each checklist item from `SKILL.md` with rationale and
before/after examples. Load it when a finding needs deeper justification.

## 1. Error handling

**Rationale:** Panics in a backend crash the request/worker. Fallible paths must
return errors callers can handle.

```rust
// before
let config = std::fs::read_to_string(path).unwrap();
// after
let config = std::fs::read_to_string(path)
    .map_err(|e| ConfigError::Read { path: path.into(), source: e })?;
```

Prefer a domain error enum with `thiserror` over `anyhow` in library crates;
reserve `anyhow` for binaries.

## 2. Ownership & borrowing

**Rationale:** Unnecessary clones cost allocations and obscure ownership.

```rust
// before
fn handle(name: String) { log(name.clone()); store(name); }
// after
fn handle(name: &str) { log(name); store(name.to_owned()); }
```

## 3. Async correctness

**Rationale:** Blocking the executor stalls every task on that worker thread.

- Wrap CPU/IO-blocking work in `tokio::task::spawn_blocking`.
- Never hold a `std::sync::Mutex`/`RwLock` guard across an `.await`; use
  `tokio::sync::Mutex` if a lock must span awaits, or restructure to drop the
  guard first.

## 4. Safety

**Rationale:** `unsafe` shifts a proof obligation to the author; it must be
documented.

```rust
// SAFETY: `ptr` is non-null and points to an initialized `T` for the
// lifetime of `self`, established at construction.
unsafe { &*ptr }
```

## 5. API surface

- Document every `pub` item (`#![warn(missing_docs)]` enforces this).
- Keep modules' public surface minimal; prefer `pub(crate)` for internals.
- Mark builder methods `#[must_use]`.

## 6. Tests

- Cover the happy path and at least one error path per public function.
- For async, use `#[tokio::test]`.
- Prefer deterministic tests (inject clocks/seeds) over time/random-dependent
  ones.
