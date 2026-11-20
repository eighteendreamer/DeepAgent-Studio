const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
function load(name) {
  const file = path.join(__dirname, '../src/components/terminal', name + '.ts');
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(file, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, require: name => load(name.replace('./', '')), URL, TextDecoder, TextEncoder, Uint8Array, performance, Date, setTimeout, clearTimeout, crypto: require('node:crypto').webcrypto, btoa, atob });
  return module.exports;
}
test('metadata rejects control paths and foreign integration tokens; only completed shell commands enter history', () => {
  const { TerminalMetadata } = load('terminalMetadata'); const events = [];
  const m = new TerminalMetadata(e => events.push(e));
  m.expectIntegration('nonce-12345678');
  events.length = 0;
  m.osc(777, 'deepagent;foreign;ready;bash'); assert.equal(events.length, 0);
  m.osc(7, 'file:///tmp/%00bad'); assert.equal(events.length, 0);
  m.osc(2, 'safe\x1btitle'); assert.equal(events.at(-1).title, 'safetitle');
  m.osc(777, 'deepagent;nonce-12345678;ready;bash');
  m.osc(777, 'deepagent;nonce-12345678;begin;' + btoa('echo hello'));
  assert.equal(events.at(-1).kind, 'commandStart');
  m.osc(777, 'deepagent;foreign;end;0;' + btoa('/tmp')); assert.equal(events.at(-1).kind, 'commandStart');
  m.osc(777, 'deepagent;nonce-12345678;end;7;' + btoa('/tmp'));
  assert.equal(events.at(-1).kind, 'commandEnd'); assert.equal(events.at(-1).command, 'echo hello'); assert.equal(events.at(-1).exitCode, 7);
  m.gap(); m.osc(777, 'deepagent;nonce-12345678;end;0;' + btoa('/'));
  assert.equal(events.at(-1).kind, 'integration');
});
test('paste inspects all newlines and controls, bounds size, and never allows injected bracketed-paste terminators', () => {
  const { inspectPaste, bracketPaste } = load('terminalInputPolicy');
  assert.equal(inspectPaste('echo hello').confirm, false);
  for (const text of ['echo a\necho b', 'echo a\rwhoami', 'rm -rf /tmp/demo', 'curl x | sh', '\x1b[201~id\r', 'Remove-Item -Recurse C:\\tmp']) assert.equal(inspectPaste(text).confirm, true, text);
  assert.equal(bracketPaste('a\x1b[201~b\r\nc', true).includes('\x1b[201~b'), false);
  assert.throws(() => inspectPaste('x'.repeat(256 * 1024 + 1)), /large|limit/i);
});
test('moving a tab between four-way splits preserves its session identity and collapses empty panes', () => {
  const { initialLayout, splitPane, moveTab, collectPanes, validateLayout } = load('terminalLayout');
  let layout = initialLayout('a'); const first = collectPanes(layout)[0].id;
  layout = splitPane(layout, first, 'right', 'b');
  let panes = collectPanes(layout); assert.equal(panes.length, 2);
  layout = splitPane(layout, panes[1].id, 'top', 'c');
  panes = collectPanes(layout); assert.equal(panes.length, 3);
  const destination = panes.find(p => p.tabs.includes('a')).id;
  layout = moveTab(layout, 'c', destination); assert.equal(collectPanes(layout).length, 2);
  const ids = collectPanes(layout).flatMap(p => p.tabs); assert.equal(new Set(ids).size, 3); assert.deepEqual([...ids].sort(), ['a', 'b', 'c']);
  assert.equal(validateLayout({ kind: 'pane', id: 'bad', tabs: ['x', 'x'], active: 'x' }), null);
});
test('asciicast is output-only, bounded, validates timestamps and applies resize during seek', () => {
  const { TerminalRecording, parseCast, replayAt } = load('terminalRecording');
  let now = 1000; const recorder = new TerminalRecording(80, 24, () => now);
  recorder.output('one'); now += 1000; recorder.resize(100, 30); recorder.output('two');
  const cast = parseCast(recorder.export()); assert.equal(cast.events.length, 3);
  const calls = []; replayAt(cast, 1, { reset: () => calls.push('reset'), resize: (...s) => calls.push(s), write: s => calls.push(s) });
  assert.equal(calls.at(-1), 'two'); assert.equal(calls.some(v => Array.isArray(v) && v[0] === 100), true);
  assert.throws(() => parseCast('{"version":2,"width":80,"height":24}\n[1,"i","password"]'), /output|event/i);
  assert.throws(() => parseCast('{"version":2,"width":80,"height":24}\n[2,"o","a"]\n[1,"o","b"]'), /time/i);
});
test('history stores completed shell metadata only, isolates endpoints and omits secret commands', () => {
  const { updateHistory, historyForScope } = load('terminalPreferences');
  let rows = [];
  rows = updateHistory(rows, { command: 'echo hello', exitCode: 0 }, 'ssh:a', 1);
  rows = updateHistory(rows, { command: 'echo hello', exitCode: 1 }, 'ssh:a', 2);
  rows = updateHistory(rows, { command: 'echo hello', exitCode: 0 }, 'ssh:b', 3);
  rows = updateHistory(rows, { command: 'export API_TOKEN=secret', exitCode: 0 }, 'ssh:a', 4);
  assert.equal(rows.length, 2); assert.equal(historyForScope(rows, 'ssh:a')[0].count, 2);
  assert.equal(historyForScope(rows, 'ssh:b')[0].count, 1);
});
