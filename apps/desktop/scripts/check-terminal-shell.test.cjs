const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const { spawnSync } = require('node:child_process');
const { terminalLoader } = require('./terminal-test-loader.cjs');
const nonce = '12345678-1234-1234-1234-123456789abc';
const bash = process.env.DA_TEST_BASH || (process.platform === 'win32' ? 'C:/Program Files/Git/bin/bash.exe' : '/bin/bash');
const { shellIntegrationCommand } = terminalLoader()('terminalShellIntegration');
function observe(setup, commands) {
  const input = setup+'\n'+shellIntegrationCommand('bash',nonce).replace(/\r$/,'\n')+commands.join('\n')+'\nexit\n';
  const result=spawnSync(bash,['--noprofile','--norc','-i'],{input,encoding:'utf8',timeout:5000,env:{...process.env,TERM:'dumb'}});
  assert.equal(result.error,undefined);
  return [...result.stdout.matchAll(/\x1b\]777;deepagent;[^;]+;([^\x07]+)\x07/g)].map(match=>match[1]);
}
test('Bash hooks preserve existing prompt commands and report exit/cwd without recording password input', {skip:!fs.existsSync(bash)},()=>{
  for (const setup of ["PROMPT_COMMAND='printf existing-prompt'", "PROMPT_COMMAND=('printf first-prompt' 'printf second-prompt')"]) {
    const events=observe(setup,['echo hello','false','cd /tmp','read -s -p Password value','fixture-password-input','echo after']);
    assert.ok(events.includes('ready;bash'));
    const starts=events.filter(e=>e.startsWith('begin;')).map(e=>Buffer.from(e.slice(6),'base64').toString('utf8'));
    assert.ok(starts.includes('echo hello')); assert.ok(starts.includes('false')); assert.ok(starts.includes('echo after'));
    assert.equal(starts.some(command=>command.includes('fixture-password-input')),false);
    assert.ok(events.some(e=>e.startsWith('end;1;'))); assert.ok(events.some(e=>e.endsWith(';L3RtcA==')));
  }
});
test('Bash refuses incompatible DEBUG hooks and does not collect ignored history lines', {skip:!fs.existsSync(bash)},()=>{
  assert.ok(observe("trap ':' DEBUG",['echo test']).some(e=>e==='unsupported'));
  const events=observe('HISTCONTROL=ignorespace',['echo visible',' echo ignored','echo tail']);
  const starts=events.filter(e=>e.startsWith('begin;')).map(e=>Buffer.from(e.slice(6),'base64').toString('utf8'));
  assert.equal(starts.filter(e=>e==='echo visible').length,1); assert.equal(starts.includes(' echo ignored'),false);
});
test('integration validates its nonce and keeps shell bootstraps below the line limit',()=>{
  assert.throws(()=>shellIntegrationCommand('bash',"'; bad"), /nonce/);
  assert.ok(shellIntegrationCommand('bash',nonce).length<3900);
  assert.ok(shellIntegrationCommand('zsh',nonce).length<3900);
  assert.ok(shellIntegrationCommand('powershell',nonce).length<3900);
});
test('PowerShell wrapper preserves the existing history privacy decision and emits only accepted command boundaries', () => {
  const command = shellIntegrationCommand('powershell',nonce).replace(/\r$/, '\n');
  const script = "Import-Module PSReadLine -ErrorAction Stop\nSet-PSReadLineOption -HistorySaveStyle SaveNothing -AddToHistoryHandler {param($line) if($line -eq 'private'){return 'SkipAdding'}; return 'MemoryAndFile'}\n" + command + "\n$handler=(Get-PSReadLineOption).AddToHistoryHandler\n$accepted=$handler.Invoke('echo fixture')\n$null=prompt\n$filtered=$handler.Invoke('private')\nif([string]$accepted -ne 'MemoryAndFile' -or [string]$filtered -ne 'SkipAdding'){throw 'history decision changed'}\n";
  const encoded=Buffer.from(script,'utf16le').toString('base64');
  const result=spawnSync(process.env.DA_TEST_PWSH || 'pwsh',['-NoProfile','-NonInteractive','-EncodedCommand',encoded],{encoding:'utf8',timeout:10000});
  assert.equal(result.error,undefined); assert.equal(result.status,0,result.stderr);
  const starts=[...result.stdout.matchAll(/;begin;([^\x07]+)\x07/g)].map(match=>Buffer.from(match[1],'base64').toString('utf8'));
  assert.deepEqual(starts,['echo fixture']); assert.ok(result.stdout.includes(';end;0;'));
});
