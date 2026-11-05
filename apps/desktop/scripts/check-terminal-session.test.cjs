const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const api = readFileSync(path.join(__dirname, '../src/api.ts'), 'utf8');
const ast = ts.createSourceFile('api.ts', api, ts.ScriptTarget.Latest, true);

function production(name, invoke) {
  const declaration = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === name);
  assert.ok(declaration, name);
  const source = `${declaration.getText(ast).replace(/^export\s+/, '')}\n${name};`;
  const js = ts.transpileModule(source, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS,
  } }).outputText;
  return vm.runInNewContext(js, { getInvoke: () => invoke });
}

const session = { sessionId: 'synthetic-pty', runId: 'synthetic-run', backend: 'direct', cols: 80, rows: 24 };
const lease = { sessionId: session.sessionId, runId: session.runId, leaseId: 'synthetic-lease', epoch: 2, holder: 'user' };

test('terminal renewal invokes the registered command with the complete generation', async () => {
  const calls = [];
  const fn = production('terminalSessionRenew', async (command, args) => { calls.push({ command, args }); });
  await fn(session, lease);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, 'terminal_session_renew');
  assert.equal(calls[0].args.session, session);
  assert.equal(calls[0].args.lease, lease);
  const rust = readFileSync(path.join(__dirname, '../src-tauri/src/lib.rs'), 'utf8');
  assert.match(rust, /#\[tauri::command\]\s*async fn terminal_session_renew\(/);
  assert.match(rust, /generate_handler!\[[\s\S]*?\bterminal_session_renew,/);
});

test('terminal renewal never succeeds on a missing transport or a rejected generation', async () => {
  await assert.rejects(production('terminalSessionRenew', null)(session, lease), /desktop app/);
  const stale = new Error('stale-generation');
  await assert.rejects(production('terminalSessionRenew', async () => { throw stale; })(session, lease), error => error === stale);
});

test('recovery distinguishes a live process from expired input control', async () => {
  const status = { cursor: 128, available: true, controlAvailable: false };
  const fn = production('terminalSessionRecoveryStatus', async (command, args) => {
    assert.equal(command, 'terminal_session_recovery_status');
    assert.equal(args.sessionId, session.sessionId);
    return status;
  });
  assert.equal(await fn(session.sessionId), status);
  const fallback = await production('terminalSessionRecoveryStatus', null)(session.sessionId);
  assert.equal(fallback.available, false);
  assert.equal(fallback.controlAvailable, false);
});

test('explicit takeover supplies the new generation to subsequent shared close', async () => {
  const next = { ...lease, leaseId: 'new-synthetic-lease', epoch: 3 };
  const calls = [];
  const invoke = async (command, args) => { calls.push({ command, args }); return command.endsWith('takeover') ? next : undefined; };
  const recovered = await production('terminalSessionTakeover', invoke)(session, 'user');
  await production('terminalSessionClose', invoke)(session, recovered);
  assert.equal(calls[0].command, 'terminal_session_takeover');
  assert.equal(calls[0].args.holder, 'user');
  assert.equal(calls[1].command, 'terminal_session_close');
  assert.equal(calls[1].args.lease, next);
  assert.notEqual(calls[1].args.lease.leaseId, lease.leaseId);
});
