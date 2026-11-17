const assert = require('node:assert/strict');
const { test } = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');
const root = path.resolve(__dirname, '..');

function observer() {
  const file = path.join(root, 'src/remoteTransferObserver.ts');
  assert.ok(fs.existsSync(file), 'production shared transfer observer must exist');
  const timers = new Map(); let id = 0;
  const module = { exports: {} };
  vm.runInNewContext(ts.transpileModule(fs.readFileSync(file, 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText, {
    exports: module.exports, module,
    require: () => ({ remoteTransfersWatch: () => { throw Error('unexpected singleton request'); } }),
    setTimeout: (fn, ms) => { timers.set(++id, { fn, ms }); return id; },
    clearTimeout: key => timers.delete(key),
  });
  const calls = [];
  const subscribe = module.exports.createTransferObserver(cursor => new Promise((resolve, reject) => calls.push({ cursor, resolve, reject })));
  return { exports: module.exports, subscribe, calls, timers, tick() { const [key, timer] = timers.entries().next().value; timers.delete(key); timer.fn(); return timer.ms; } };
}
const flush = () => new Promise(setImmediate);
const snapshot = (revision, state = 'running') => ({ revision, tasks: [{ id: 'task', attempt: 1, state, request: {connectionId: 'a', transfer: {direction:'upload', remotePath:'/file'}} }] });

test('panel and file tree share one wait, including StrictMode detach/reattach', async () => {
  const h = observer(); const received = []; const stale = [];
  const off = h.subscribe({ next: value => stale.push(value), error: assert.fail });
  off();
  const offPanel = h.subscribe({ next: value => received.push(value), error: assert.fail });
  const offTree = h.subscribe({ next: value => received.push(value), error: assert.fail });
  assert.equal(h.calls.length, 1);
  h.calls[0].resolve(snapshot('9007199254740993')); await flush();
  assert.equal(stale.length, 0); assert.equal(received.length, 2);
  assert.equal(h.timers.size, 1); assert.equal(h.tick(), 100);
  assert.equal(h.calls[1].cursor, '9007199254740993', 'cursor cannot be rounded by Number');
  h.calls[1].resolve(snapshot('9007199254740993')); await flush();
  assert.equal(received.length, 2, 'idle heartbeat must not repaint');
  offPanel(); offTree(); assert.equal(h.timers.size, 0);
});

test('slow reads never overlap; terminal notices deliver before next wait, errors back off and resync', async () => {
  const h = observer(); const values = []; const errors = [];
  const off = h.subscribe({ next: v => values.push(v), error: e => errors.push(String(e)) });
  await flush(); assert.equal(h.calls.length, 1); assert.equal(h.timers.size, 0);
  h.calls[0].resolve(snapshot('1')); await flush(); h.tick();
  h.calls[1].resolve(snapshot('2', 'completed')); await flush();
  assert.equal(values[1].tasks[0].state, 'completed');
  h.tick(); h.calls[2].reject(Error('reload host')); await flush();
  assert.deepEqual(errors, ['Error: reload host']); assert.equal(h.tick(), 1000);
  assert.equal(h.calls[3].cursor, undefined, 'failure must force a fresh snapshot');
  off(); h.calls[3].resolve(snapshot('3')); await flush();
  assert.equal(values.length, 2); assert.equal(h.timers.size, 0);
});

test('watch IPC carries exact revision and refuses missing desktop bridge', async () => {
  const source = fs.readFileSync(path.join(root, 'src/remoteTransfersApi.ts'), 'utf8');
  const module = { exports: {} }; const calls = []; let invoke = async (...args) => {calls.push(args); return snapshot('4');};
  vm.runInNewContext(ts.transpileModule(source, {compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.CommonJS}}).outputText, {
    exports:module.exports,module,require:()=>({getInvoke:()=>invoke}),
  });
  assert.equal(typeof module.exports.remoteTransfersWatch, 'function');
  await module.exports.remoteTransfersWatch('9007199254740993');
  assert.equal(calls[0][0], 'remote_transfer_watch');
  assert.equal(calls[0][1].afterRevision, '9007199254740993');
  invoke = null;
  await assert.rejects(module.exports.remoteTransfersWatch(), /desktop app/);
});

test('published uploads invalidate grouped parent directories once per attempt and connection', () => {
  const h = observer(); const refreshed = [];
  assert.equal(typeof h.exports.createUploadInvalidator, 'function');
  const accept = h.exports.createUploadInvalidator('a', paths => refreshed.push(Array.from(paths)));
  const task = (id, overrides = {}) => ({id, attempt:1, state:'completed', receipt:{skipped:false}, request:{connectionId:'a',transfer:{direction:'upload',remotePath:'/dir/file'}}, ...overrides});
  const first = task('first'); const second = task('second');
  accept({revision:'1',tasks:[first,second,task('skipped',{receipt:{skipped:true}}),task('failed',{state:'failed'}),task('other',{request:{connectionId:'b',transfer:{direction:'upload',remotePath:'/secret/file'}}}),task('download',{request:{connectionId:'a',transfer:{direction:'download',remotePath:'/download'}}})]});
  assert.deepEqual(refreshed, [['/dir']]);
  accept({revision:'2',tasks:[first,second]}); assert.equal(refreshed.length,1);
  accept({revision:'3',tasks:[first,task('second',{attempt:2})]});
  assert.deepEqual(refreshed, [['/dir'],['/dir']]);
});
