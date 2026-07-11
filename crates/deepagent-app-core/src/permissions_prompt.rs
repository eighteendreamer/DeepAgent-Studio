//! Generates the sandbox/permissions system-prompt section injected into the
//! dynamic portion of the model's system prompt. This tells the model what its
//! current sandbox constraints are so it doesn't attempt blocked operations or
//! retry indefinitely after a denial.
//!
//! Design mirrors Codex's `templates/permissions/sandbox_mode/*.md`.

use crate::settings::SandboxMode;

/// Generate the sandbox & permissions instruction block for the given mode.
pub fn sandbox_instructions(mode: SandboxMode) -> String {
    match mode {
        SandboxMode::ReadOnly => READ_ONLY_INSTRUCTIONS.to_string(),
        SandboxMode::WorkspaceWrite => WORKSPACE_WRITE_INSTRUCTIONS.to_string(),
        SandboxMode::FullAccess => FULL_ACCESS_INSTRUCTIONS.to_string(),
    }
}

const READ_ONLY_INSTRUCTIONS: &str = "\
# Sandbox & Permissions

Current sandbox mode: **read-only**

The sandbox only permits reading files. All file-write operations are blocked at the OS level — this includes:
- The `write_file` / `edit_file` / `multi_edit` tools
- Shell commands that write files (`echo > file`, `>>`, `tee`, heredocs, `cp`, `mv`, `mkdir`, `touch`, etc.)
- Any programming-language file I/O (Python `open('w')`, Node `fs.writeFile`, etc.)

Do NOT attempt alternative write methods — they are all blocked.

If you need to write files to complete the user's request:
1. Inform the user that the current sandbox mode is read-only.
2. Suggest the user switch to a higher permission level (Workspace Write or Full Access).
3. Do NOT retry the write operation. Do NOT try workarounds.

## Escalation protocol

If a command previously failed due to sandbox restrictions and you believe it is essential:
- Re-invoke the `bash` tool with `sandbox_permissions: \"require_escalated\"` and a `justification` explaining why.
- The user will be prompted to approve. If denied, stop and inform the user.

Network access: requests require user approval.";

const WORKSPACE_WRITE_INSTRUCTIONS: &str = "\
# Sandbox & Permissions

Current sandbox mode: **workspace-write**

The sandbox permits reading files anywhere and writing files within the active project directory.
- Writes to paths inside the project root are allowed.
- Writes to paths outside the project root are blocked.

If a write is denied because the target is outside the workspace:
1. Inform the user that the path is outside the allowed workspace.
2. Suggest an alternative path within the project, or ask the user to switch to Full Access mode.
3. Do NOT retry the blocked operation.

## Escalation protocol

If a command previously failed due to sandbox restrictions and you believe it is essential:
- Re-invoke the `bash` tool with `sandbox_permissions: \"require_escalated\"` and a `justification` explaining why.
- The user will be prompted to approve. If denied, stop and inform the user.

Network access: requests require user approval.";

const FULL_ACCESS_INSTRUCTIONS: &str = "\
# Sandbox & Permissions

Current sandbox mode: **full-access**

All file operations (read and write) are permitted without restriction.
Shell commands run without sandbox confinement.
Network access is unrestricted.";
