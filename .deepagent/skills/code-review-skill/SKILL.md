---
name: Code Review
description: This skill should be used when the user asks to "review code", "review a file", "review a diff", "review a pull request", "audit code quality", "find security issues", or "check for performance problems". Performs structured code review with severity-classified findings across correctness, security, performance, and maintainability, with verification gates against false completion claims.
version: 0.1.0
---

# Code Review

Perform a structured, high-signal review of code, a file, a diff, or a pull
request. Classify findings by severity, cite concrete `file:line` references,
and refuse to claim success without verification.

## When to use

Use when reviewing changes before merge, auditing quality, or hunting for
security and performance issues across many languages (React, Vue, Rust,
TypeScript, and more).

## Review dimensions

1. **Correctness** — logic errors, edge cases, off-by-one, error handling,
   nullability, race conditions.
2. **Security** — injection (SQL/command/XSS), authn/authz gaps, secret
   leakage, unsafe deserialization, SSRF, path traversal.
3. **Performance** — N+1 queries, needless allocations/clones, blocking calls
   on hot paths, missing pagination/indexes.
4. **Maintainability** — naming, cohesion, duplication, test coverage, public
   API surface, documentation.

## Output format

Group findings by severity:

- **Blocker** — must fix before merge; include a concrete suggested diff.
- **Should-fix** — important but not merge-blocking; describe the change.
- **Nit** — minor/style; mention briefly.

End with a short verdict (approve / request-changes) and the verification you
ran (build/tests/lint). Do not assert "looks good" without having checked.

## Verification gate

Before concluding, run the project's build, tests, and linter if available, and
fold the results into the review. Never make a completion/success claim that
verification has not backed.

## Bundled resources

- `references/checklists.md` — language-specific review checklists (Rust,
  TypeScript/React, Vue) and a security sweep list.
