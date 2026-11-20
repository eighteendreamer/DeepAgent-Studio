export type IntegrationShell = "bash" | "zsh" | "powershell";
function base64(text: string): string { return btoa(String.fromCharCode(...new TextEncoder().encode(text))); }

/** Explicitly installed in the current interactive shell; no rc/profile files are written. */
export function shellIntegrationCommand(shell: IntegrationShell, nonce: string): string {
  if (!/^[a-f0-9-]{36}$/.test(nonce)) throw new Error("Invalid shell integration nonce");
  if (shell === "powershell") {
    const body = `
if (!(Get-Module PSReadLine) -or $script:__da_installed) { [Console]::Write("$([char]27)]777;deepagent;${nonce};unsupported$([char]7)"); return }
$script:__da_installed = $true
$script:__da_originalHistoryHandler = (Get-PSReadLineOption).AddToHistoryHandler
$script:__da_originalPrompt = (Get-Command prompt).ScriptBlock
$script:__da_busy = $false
function global:__da_emit($a,$b) { [Console]::Write("$([char]27)]777;deepagent;${nonce};$a;$b$([char]7)") }
function global:__da_encode($s) { [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($s)) }
Set-PSReadLineOption -AddToHistoryHandler { param($line) $result=if($script:__da_originalHistoryHandler){$script:__da_originalHistoryHandler.Invoke($line)}else{$true}; if($result -eq $true -or [string]$result -eq 'MemoryAndFile'){ __da_emit 'begin' (__da_encode $line); $script:__da_busy=$true }; return $result }
function global:prompt { $ok=$?; $status=if($ok){'0'}else{'unknown'}; $path=__da_encode $ExecutionContext.SessionState.Path.CurrentLocation.Path; if($script:__da_busy){ __da_emit 'end' "$status;$path"; $script:__da_busy=$false }; __da_emit 'prompt' "0;$path"; & $script:__da_originalPrompt }
__da_emit 'ready' 'powershell'
`;
    return ` Invoke-Expression ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${base64(body)}')))\r`;
  }
  const common = `
__da_emit() { printf '\\033]777;deepagent;${nonce};%s;%s\\007' "$1" "$2"; }
__da_encode() { printf '%s' "$1" | base64 | tr -d '\\r\\n'; }
__da_busy=0
`;
  const body = shell === "bash" ? `
if [[ -z "\${BASH_VERSION:-}" || -n "$(trap -p DEBUG)" || -n "\${__da_installed:-}" ]]; then printf '\\033]777;deepagent;${nonce};unsupported\\007'; else
${common}
__da_installed=1
__da_ready=0
__da_last_history=""
__da_preexec() { [[ "\${__da_ready:-0}" == 1 && "$BASH_COMMAND" != __da_* ]] || return 0; __da_ready=0; local line; line="$(HISTTIMEFORMAT= builtin history 1)"; [[ "$line" != "$__da_last_history" ]] || return 0; __da_last_history="$line"; if [[ "$line" =~ ^[[:space:]]*[0-9]+[[:space:]]+(.*)$ ]]; then __da_emit begin "$(__da_encode "\${BASH_REMATCH[1]}")"; __da_busy=1; fi; }
__da_prompt_begin() { local status="$1"; __da_ready=0; if [[ "$__da_busy" == 1 ]]; then __da_emit end "$status;$(__da_encode "$PWD")"; __da_busy=0; fi; __da_emit prompt "0;$(__da_encode "$PWD")"; }
__da_prompt_end() { __da_ready=1; }
PROMPT_COMMAND=('__da_prompt_begin "$?"' "\${PROMPT_COMMAND[@]}" '__da_prompt_end')
trap '__da_preexec' DEBUG
__da_emit ready bash
fi
` : `
if [[ -z "\${ZSH_VERSION:-}" || -n "\${__da_installed:-}" ]]; then printf '\\033]777;deepagent;${nonce};unsupported\\007'; else
${common}
__da_installed=1
__da_preexec() { __da_emit begin "$(__da_encode "$1")"; __da_busy=1; }
__da_precmd() { local status="$?"; if [[ "$__da_busy" == 1 ]]; then __da_emit end "$status;$(__da_encode "$PWD")"; __da_busy=0; fi; __da_emit prompt "0;$(__da_encode "$PWD")"; }
preexec_functions+=(__da_preexec)
precmd_functions+=(__da_precmd)
__da_emit ready zsh
fi
`;
  const command = ` eval "$(printf '%s' '${base64(body)}' | base64 -d)"\r`;
  if (command.length > 3900) throw new Error("Shell bootstrap exceeds the terminal line limit");
  return command;
}
