# Code Review — Checklists

Consult the relevant list while reviewing. Cite `file:line` for each finding.

## Security sweep (all languages)

- [ ] User input validated/sanitized before use.
- [ ] Parameterized queries (no string-concatenated SQL).
- [ ] No shell command built from unescaped user input.
- [ ] Output encoded to prevent XSS in web contexts.
- [ ] AuthN/AuthZ enforced on every protected path.
- [ ] No secrets in code, logs, or error messages.
- [ ] No unsafe deserialization of untrusted data.
- [ ] External requests can't be coerced to internal targets (SSRF).
- [ ] File paths confined; no traversal via `..`.

## Rust

- [ ] No `unwrap`/`expect` on fallible non-test paths.
- [ ] `unsafe` blocks carry a `// SAFETY:` justification.
- [ ] No needless `clone()`; borrows preferred.
- [ ] No `std::sync::Mutex` guard held across `.await`.
- [ ] Public items documented; errors via `thiserror`/typed enums.

## TypeScript / React

- [ ] No `any` where a real type fits; props/return types explicit.
- [ ] Effects have correct dependency arrays; no missing cleanups.
- [ ] Keys on lists stable and unique.
- [ ] No state mutation; immutable updates.
- [ ] Async UI handles loading/empty/error states.

## Vue

- [ ] Reactive state via `ref`/`reactive`; no direct prop mutation.
- [ ] `computed` for derived state instead of recomputing in template.
- [ ] `v-for` has stable `:key`; `v-if`/`v-for` not on same node.
- [ ] Cleanup in `onUnmounted` for listeners/timers.

## Performance

- [ ] No N+1 queries; batch or join.
- [ ] Pagination on unbounded lists.
- [ ] Hot loops avoid per-iteration allocation.
- [ ] Heavy work off the request/UI thread.
