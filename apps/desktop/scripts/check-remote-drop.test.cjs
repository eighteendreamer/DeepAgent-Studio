const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const vm = require('node:vm');
const ts = require('typescript');
const root = path.resolve(__dirname, '..');
const source = readFileSync(path.join(root, 'src/components/remote/RemoteSidebar.tsx'), 'utf8');
const ast = ts.createSourceFile('sidebar.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const errorsSource = readFileSync(path.join(root, 'src/components/settings/ConnectionsSettings.tsx'), 'utf8');
const errorsAst = ts.createSourceFile('settings.tsx', errorsSource, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const normalize = errorsAst.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === 'normalizeSshError');
const normalizeSshError = vm.runInNewContext(ts.transpileModule(normalize.getText(errorsAst).replace(/^export /, '') + '\nnormalizeSshError;', {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText);

async function harness({ status = 'connected', target = '/目标', submit } = {}) {
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === 'useEffect' &&
        node.arguments[0]?.getText(ast).includes('setupDragDrop')) effect = node.arguments[0];
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(effect, 'exercise the native subscription used by the production sidebar');
  const calls = [], notices = [], reads = [], errors = [], points = [];
  const selected = { current: { id: 'host-a', status } };
  let callback, unlistens = 0;
  const text = ts.transpileModule('const effect = ' + effect.getText(ast) + '; effect;', {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const fn = vm.runInNewContext(text, {
    require: name => {
      assert.equal(name, '@tauri-apps/api/webview');
      return { getCurrentWebview: () => ({ onDragDropEvent: async cb => {
        callback = cb; return () => unlistens++;
      } }) };
    },
    selectedRef: selected,
    document: { elementFromPoint: (x, y) => { points.push([x, y]); return {
      closest: () => target === null ? null : { dataset: { remoteDropDir: target } },
    }; } },
    window: { devicePixelRatio: 2 },
    remoteTransferSubmit: async (...args) => { calls.push(args); return submit?.(...args); },
    loadDir: async (...args) => reads.push(args),
    message: { success: s => notices.push(['success', s]), error: s => notices.push(['error', s]) },
    setDirError: s => errors.push(s), normalizeSshError, Error,
    t: (key, args) => key + (args ? ':' + args.count : ''),
  });
  const cleanup = fn();
  await new Promise(setImmediate);
  assert.ok(callback);
  return { calls, notices, reads, errors, selected, points, cleanup, unlistens: () => unlistens,
    drop: paths => callback({ payload: { type: 'drop', paths, position: { x: 100, y: 80 } } }) };
}

test('mixed real file/directory drop preserves exact paths, asks backend to detect kind and waits for completion refresh', async () => {
  const temp = mkdtempSync(path.join(os.tmpdir(), 'deepagent-drop-'));
  try {
    const file = path.join(temp, 'same');
    const directory = path.join(temp, 'nested', 'same');
    writeFileSync(file, 'file'); mkdirSync(directory, { recursive: true });
    const h = await harness();
    await h.drop([file, directory]);
    assert.deepEqual(h.calls.map(c => c[1].localPath), [file, directory]);
    assert.deepEqual(h.calls.map(c => c[1].remotePath), ['/目标/same', '/目标/same']);
    for (const call of h.calls) {
      assert.equal(call[0], 'host-a');
      assert.equal(call[1].direction, 'upload');
      assert.equal(call[2]?.detectLocalKind, true);
      assert.equal(call[1].conflict ?? 'ask', 'ask');
    }
    assert.deepEqual(h.points, [[50, 40]], 'native physical position maps to CSS pixels once');
    assert.equal(h.reads.length, 0, 'enqueued is not published: shared completion observer owns refresh');
    assert.deepEqual(h.notices, [['success', 'remote.transfers.enqueued:2']]);
    h.cleanup(); assert.equal(h.unlistens(), 1);
  } finally { rmSync(temp, { recursive: true, force: true }); }
});

test('drop outside remote tree or while disconnected queues nothing', async () => {
  for (const options of [{ target: null }, { status: 'disconnected' }, { status: 'connecting' }]) {
    const h = await harness(options);
    await h.drop(['D:\\drop\\file']);
    assert.equal(h.calls.length, 0);
    assert.equal(h.reads.length, 0);
    h.cleanup();
  }
});

test('one rejected path retains error context and does not prevent later sources entering queue', async () => {
  const h = await harness({ submit: async (_id, input) => {
    if (input.localPath.includes('missing')) throw Error('local source not found');
  } });
  await h.drop(['D:\\drop\\missing', 'D:\\drop\\folder\\']);
  assert.equal(h.calls.length, 2);
  assert.equal(h.calls[1][1].remotePath, '/目标/folder');
  assert.match(h.notices.find(n => n[0] === 'error')?.[1] ?? '', /local source not found/);
  assert.ok(h.notices.some(n => n[1] === 'remote.transfers.enqueued:1'));
  h.cleanup();
});

test('changing selected host or unmounting fences remaining submissions in a pending drop', async () => {
  for (const change of ['host', 'unmount']) {
    let release;
    const h = await harness({ submit: () => new Promise(resolve => { release = resolve; }) });
    const pending = h.drop(['D:\\drop\\a', 'D:\\drop\\b']);
    assert.equal(h.calls.length, 1);
    if (change === 'host') h.selected.current = { id: 'host-b', status: 'connected' };
    else h.cleanup();
    release();
    // Baseline may start a second pending submission; settle it to avoid a hanging test.
    await new Promise(setImmediate);
    release(); await pending;
    assert.equal(h.calls.length, 1);
    assert.equal(h.reads.length, 0);
    if (change === 'host') h.cleanup();
  }
});

test('detection is an opt-in argument to the existing command, not persisted in request DTO', async () => {
  const api = readFileSync(path.join(root, 'src/remoteTransfersApi.ts'), 'utf8');
  const apiAst = ts.createSourceFile('api.ts', api, ts.ScriptTarget.Latest, true);
  const fn = apiAst.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === 'remoteTransferSubmit');
  const calls = [];
  const submit = vm.runInNewContext(ts.transpileModule(fn.getText(apiAst).replace(/^export /, '') + '\nremoteTransferSubmit;', {
    compilerOptions: { target: ts.ScriptTarget.ES2022 },
  }).outputText, { getInvoke: () => async (cmd, args) => calls.push({ cmd, args }) });
  await submit('host', { direction: 'upload', localPath: 'D:/folder', remotePath: '/folder' }, { detectLocalKind: true });
  assert.equal(calls[0].cmd, 'remote_transfer_submit');
  assert.equal(calls[0].args.detectLocalKind, true);
  assert.equal('detectLocalKind' in calls[0].args.request, false);
  assert.equal(calls[0].args.request.directoryConflict, 'ask');
  await submit('host', { direction: 'download', localPath: 'D:/file', remotePath: '/file' });
  assert.equal(calls[1].args.detectLocalKind ?? false, false);
});
