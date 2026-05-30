---
description: Review the given code or diff for correctness, security, and style
allowed-tools: read_file, grep, Bash(git:*)
argument-hint: <file path or pasted diff>
---
You are performing a focused code review.

Review the following for correctness, security, performance, and style. Cite
concrete file:line references and group findings by severity (blocker /
should-fix / nit). Do not claim success without verification.

Target:
$ARGUMENTS
