# Superpowers — Disciplined Workflow

A repeatable loop for non-trivial work. Each phase has an explicit exit
checkpoint; do not advance until it is met.

## 1. Brainstorm

- Restate the goal in one sentence.
- List 2–3 candidate approaches with tradeoffs.
- Pick one and record *why*.

**Checkpoint:** chosen approach + rationale written down.

## 2. Plan

- Break the work into small, ordered, independently verifiable steps.
- Identify the riskiest step; consider de-risking it first.
- Note what "done" looks like for the whole task.

**Checkpoint:** a step list exists, each step testable.

## 3. Test-first

- For each step, write (or describe) the failing test that defines success.
- Prefer deterministic tests; inject clocks/seeds.

**Checkpoint:** failing tests exist and express intent.

## 4. Implement

- Make the smallest change that turns one test green.
- Keep the build green between steps; commit logically.

**Checkpoint:** all targeted tests pass; build/lint clean.

## 5. Review

- Re-read the diff against the original goal.
- Check edge cases, error paths, and naming.
- Run the full quality gate (build, tests, lint, format).

**Checkpoint:** review notes addressed; gates green.

## Anti-patterns

- Jumping to code before the plan exists.
- Declaring success without running verification.
- Large, unreviewable changes that mix concerns.
