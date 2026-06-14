---
name: superpowers
description: Use when starting any non-trivial development work — bundles brainstorming, plan writing, plan execution, TDD, systematic debugging, verification, code review (requesting & receiving), parallel agent dispatch, subagent-driven dev, git worktrees, branch completion, and skill authoring. The 14 sub-skills live under skills/<sub>/SKILL.md inside this skill's base_dir.
---

# Superpowers

This skill is a **bundle / index**, not a workflow itself. It registers a
single catalog entry covering 14 specialized agent sub-skills sourced from
the upstream [obra/superpowers](https://github.com/obra/superpowers) project,
and points you at the specific sub-skill's `SKILL.md` you need for any given
task.

You MUST `read_file` the relevant sub-skill's `SKILL.md` (relative to this
skill's `base_dir`) before acting on its domain — this index does not
substitute for the sub-skill's own instructions.

## Sub-skill index

Pick the entry whose "Use when" matches the work in front of you, then read
the file in the right column.

- `skills/using-superpowers/SKILL.md` — Use when starting any conversation;
  establishes how to find and use skills.
- `skills/brainstorming/SKILL.md` — Use before any creative work (creating
  features, building components, modifying behavior); explores intent and
  requirements before implementation.
- `skills/writing-plans/SKILL.md` — Use when you have a spec or requirements
  for a multi-step task, before touching code.
- `skills/executing-plans/SKILL.md` — Use when you have a written plan to
  execute in a separate session with review checkpoints.
- `skills/subagent-driven-development/SKILL.md` — Use when executing
  implementation plans with independent tasks in the current session.
- `skills/dispatching-parallel-agents/SKILL.md` — Use when facing 2+
  independent tasks that can be worked on in parallel without shared state.
- `skills/test-driven-development/SKILL.md` — Use when implementing any
  feature or bugfix, before writing implementation code.
- `skills/systematic-debugging/SKILL.md` — Use when encountering any bug,
  test failure, or unexpected behavior, before proposing fixes.
- `skills/verification-before-completion/SKILL.md` — Use when about to claim
  work is complete, fixed, or passing, before committing or opening PRs.
- `skills/requesting-code-review/SKILL.md` — Use when completing tasks,
  implementing major features, or before merging.
- `skills/receiving-code-review/SKILL.md` — Use when receiving code review
  feedback, before implementing suggestions.
- `skills/using-git-worktrees/SKILL.md` — Use when starting feature work
  that needs isolation from the current workspace.
- `skills/finishing-a-development-branch/SKILL.md` — Use when implementation
  is complete and you need to decide how to integrate the work.
- `skills/writing-skills/SKILL.md` — Use when creating new skills, editing
  existing skills, or verifying skills work before deployment.

## How to consult a sub-skill

```
read_file(<base_dir>/skills/<sub-skill-id>/SKILL.md)
```

Each sub-skill's directory may also carry its own `references/`, `examples/`,
`scripts/`, and `assets/` subdirs — read or execute those by absolute path
as the sub-skill instructs.

## Provenance & license

Sub-skills are vendored from
[obra/superpowers](https://github.com/obra/superpowers). See `LICENSE` and
`README.md` in this directory for the upstream attribution and license terms.
