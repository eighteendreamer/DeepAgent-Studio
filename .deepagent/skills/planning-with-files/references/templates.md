# Planning With Files — Templates

Copy these into the working directory at task start.

## task_plan.md

```markdown
# Task: <one-line goal>

Status: in_progress
Updated: <timestamp>

## Phase 1: <name>
- [ ] step 1
- [ ] step 2

## Phase 2: <name>
- [ ] step 1

## Error log
- (none yet)
```

Rules:
- Exactly one phase should be active at a time.
- Tick checkboxes as steps complete; never delete completed items (history).
- Append failures to the Error log with the phase and a one-line cause.

## notes.md

```markdown
# Notes

## Findings
- <fact or constraint discovered>

## Decisions
- <decision> — because <rationale>

## Dead ends
- <approach tried> — abandoned because <reason>
```

## Resume protocol

On a new turn or after context compaction:
1. Read `task_plan.md` to find the active phase and unchecked steps.
2. Skim `notes.md` Decisions for constraints already settled.
3. Continue from the first unchecked step — do not redo completed work.
