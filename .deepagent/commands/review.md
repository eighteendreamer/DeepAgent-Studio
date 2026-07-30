---
description: Review code changes (uncommitted / base branch / commit / custom) with prioritized findings
allowed-tools: read_file, grep, glob, list_dir, Bash(git:*), Bash(ocr:*)
argument-hint: [base <branch> | commit <sha> | <file path or pasted diff>]（留空默认审查未提交变更）
---
You are performing a focused code review. Follow the workflow in the
`deepagent-code-review` skill (activate it via the skill tool if available;
otherwise apply the same discipline directly).

Scope resolution for the target below:
- Empty target → review uncommitted changes (`git diff HEAD` + untracked files).
- `base <branch>` → diff against `git merge-base HEAD <branch>`.
- `commit <sha>` → review `git show <sha>`.
- Anything else → treat as a custom target (file paths or a pasted diff).

Output prioritized findings: each with a `[P0]`-`[P3]` prefixed title, a concise
one-paragraph rationale with trigger conditions, and `file:line` references.
Group by severity (P0/P1 always report; P2 with context; P3 only if clearly
valuable). Discard likely false positives silently. End with an overall
correctness verdict (correct / incorrect + 1-3 sentence justification, judged
only on blocking issues). Do not claim success without verification. Do not
modify code unless the user explicitly asked for review-and-fix.

Target:
$ARGUMENTS
