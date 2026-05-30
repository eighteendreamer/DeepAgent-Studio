---
name: Planning With Files
description: This skill should be used when starting a "complex task", "multi-step project", or "research task", or when the user mentions "planning", "organizing work", "tracking progress", or wants structured output. Implements Manus-style persistent markdown planning — creates task_plan.md, notes.md, and a deliverable file so working state survives context resets.
version: 0.1.0
---

# Planning With Files

Treat the model's context window as volatile RAM and the filesystem as
persistent disk. For any complex task, write important state to disk so work can
resume without losing goals, findings, or progress.

## When to use

Use for complex, multi-step, or research tasks — anything likely to exceed a
handful of tool calls or span multiple turns where losing context would be
costly.

## The three-file pattern

1. **`task_plan.md`** — phases with checkboxes, current status, and an error
   log. The single source of truth for "what's left".
2. **`notes.md`** — research, findings, decisions, and dead ends.
3. **`<deliverable>.md`** (or code) — the actual output being produced.

## Workflow

1. At task start, scaffold `task_plan.md` with phases and checkboxes.
2. Before each step, read `task_plan.md` to re-anchor on the goal.
3. After each step, update checkboxes and append to `notes.md`.
4. On error, log it under the relevant phase rather than only reacting.
5. On resume (new turn / after compaction), reconstruct state from the files.

## Notes

- Keep `task_plan.md` terse and scannable; it is read often.
- Persist *decisions and rationale*, not just status — future-you needs the why.

## Bundled resources

- `references/templates.md` — ready-to-copy `task_plan.md` and `notes.md`
  templates.
