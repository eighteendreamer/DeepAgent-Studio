---
name: Superpowers
description: This skill should be used when the user wants to "write a skill", "create a new skill", "improve a skill", or asks how to structure skills, apply progressive disclosure, or follow disciplined engineering workflows (brainstorming, planning, test-driven development) before implementing. A meta-skill for authoring high-quality skills and following rigorous development process.
version: 0.1.0
---

# Superpowers

A meta-skill for authoring effective skills and following a disciplined
engineering workflow. Use it to turn vague intentions into well-structured
skills with strong trigger descriptions and progressive disclosure, and to keep
implementation work rigorous (brainstorm → plan → test-first → implement →
review).

## When to use

Use when creating or improving a skill, or when starting a non-trivial task that
benefits from explicit process discipline rather than jumping straight to code.

## Skill authoring checklist

1. **Concrete examples** — collect real user phrasings that should trigger it.
2. **Frontmatter** — third-person `description` packed with exact trigger
   phrases; `name` clear and specific.
3. **Lean body** — imperative voice, 1,500–2,000 words; move depth to
   `references/`.
4. **Progressive disclosure** — metadata always resident, body on activation,
   resources on demand.
5. **Validate** — triggers fire on expected queries; referenced files exist.

## Development discipline

- **Brainstorm** the approach and alternatives before committing.
- **Plan** the steps; write them down.
- **Test first** where feasible; let failing tests define "done".
- **Implement** in small, verifiable increments.
- **Review** against the original intent and quality gates.

## Bundled resources

- `references/workflow.md` — the brainstorm → plan → TDD → review loop in
  detail, with checkpoints.
