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
  const timers = new Map(); let id = 0; const diagnostics = [];
  const module = { exports: {} };
  vm.runInNewContext(ts.transpileModule(fs.readFileSync(file, 'utf8'), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText, {
    exports: module.exports, module,
    require: () => ({ remoteTransfersWatch: () => { throw Error('unexpected singleton request'); } }),
    setTimeout: (fn, ms) => { timers.set(++id, { fn, ms }); return id; },
    clearTimeout: key => timers.delete(key),
    console: { error: (...args) => diagnostics.push(args) },
  });
  const calls = [];
  const subscribe = module.exports.createTransferObserver(cursor => new Promise((resolve, reject) => calls.push({ cursor, resolve, reject })));
  return { exports: module.exports, subscribe, calls, timers, diagnostics, tick() { const [key, timer] = timers.entries().next().value; timers.delete(key); timer.fn(); return timer.ms; } };
}
const flush = () => new Promise(setImmediate);
const snapshot = (revision, state = 'running') => ({ revision, tasks: [{ id: 'task', attempt: 1, state, request: {connectionId: 'a', transfer: {direction:'upload', remotePath:'/file'}} }] });
test('keep-both invalidates the actual renamed directory on success and partial failure', () => {
  const h = observer(); const refreshed = [];
  const accept = h.exports.createUploadInvalidator('a', paths => refreshed.push(Array.from(paths)),
    () => ['/tree (1)/loaded','/tree/old']);
  const task = {id:'copy',attempt:1,state:'completed',directoryReceipt:{destinationPath:'/tree (1)'},
    request:{kind:'directory',connectionId:'a',transfer:{direction:'upload',remotePath:'/tree'}}};
  accept({revision:'1',tasks:[task]});
  assert.ok(refreshed[0].includes('/tree (1)/loaded'));
  assert.ok(!refreshed[0].includes('/tree/old'));
  accept({revision:'2',tasks:[{...task,attempt:2,state:'failed',directoryReceipt:null,
    error:{destinationPath:'/tree (2)',relativePath:'nested/bad'}}]});
  assert.ok(refreshed[1].includes('/tree (2)/nested'));
});
test('directory publication and partial failure invalidate the root and loaded descendant directories', () => {
  const h = observer(); const refreshed = [];
  const accept = h.exports.createUploadInvalidator('a', paths => refreshed.push(Array.from(paths)));
  const task = {id:'directory',attempt:1,state:'failed',completedFiles:1,
    error:{relativePath:'nested/bad'},request:{kind:'directory',connectionId:'a',transfer:{direction:'upload',remotePath:'/tree'}}};
  accept({revision:'1',tasks:[task]});
  assert.equal(refreshed.length,1);
  assert.ok(refreshed[0].includes('/'));
  assert.ok(refreshed[0].includes('/tree'));
  assert.ok(refreshed[0].includes('/tree/nested'));
  accept({revision:'2',tasks:[task]});
  assert.equal(refreshed.length,1,'terminal attempt invalidated once');
});

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

test('a failed subscriber cannot turn a valid terminal snapshot into a shared IPC failure', async () => {
  const h = observer(); const received = []; const failures = []; const healthyErrors = [];
  const offBroken = h.subscribe({
    next: () => { throw Error('fixture render failed'); },
    error: error => failures.push(error.message),
  });
  const offHealthy = h.subscribe({ next: value => received.push(value), error: error => healthyErrors.push(error) });
  h.calls[0].resolve(snapshot('generation:9007199254740993', 'completed')); await flush();
  assert.equal(received.length, 1, 'every healthy subscriber must receive the terminal result');
  assert.equal(received[0].tasks[0].state, 'completed');
  assert.deepEqual(failures, ['fixture render failed']);
  assert.equal(healthyErrors.length, 0, 'a view callback error is not an IPC failure');
  assert.equal(h.tick(), 100, 'valid snapshots retain the ordinary progress throttle');
  assert.equal(h.calls[1].cursor, 'generation:9007199254740993', 'callback failure cannot discard the shared cursor');
  offBroken(); offHealthy(); h.calls[1].resolve(snapshot('generation:9007199254740993')); await flush();
  assert.equal(h.timers.size, 0);
});

test('cached replay reports its own callback failure and still returns a working unsubscribe', async () => {
  const h = observer(); const received = []; const failures = [];
  const offFirst = h.subscribe({ next: value => received.push(value), error: assert.fail });
  h.calls[0].resolve(snapshot('cached')); await flush();
  const offBroken = h.subscribe({
    next: () => { throw Error('fixture cached callback failed'); },
    error: error => failures.push(error.message),
  });
  assert.equal(typeof offBroken, 'function', 'a failed replay cannot strand a registered subscriber');
  assert.deepEqual(failures, ['fixture cached callback failed']);
  assert.equal(h.calls.length, 1);
  offBroken(); offFirst();
  assert.equal(h.timers.size, 0);
});

test('one subscriber error handler cannot prevent IPC failure delivery or recovery for other views', async () => {
  const h = observer(); const healthyErrors = []; const recovered = [];
  const offBroken = h.subscribe({ next: () => {}, error: () => { throw Error('fixture error handler failed'); } });
  const offHealthy = h.subscribe({ next: value => recovered.push(value), error: error => healthyErrors.push(error.message) });
  h.calls[0].reject(Error('fixture IPC disconnected')); await flush();
  assert.deepEqual(healthyErrors, ['fixture IPC disconnected']);
  assert.equal(h.diagnostics.length, 1, 'a failed error handler leaves a bounded diagnostic');
  assert.equal(h.diagnostics[0].length, 1, 'diagnostic does not copy the task snapshot or error payload');
  assert.equal(h.tick(), 1000, 'actual IPC failure still backs off');
  assert.equal(h.calls[1].cursor, undefined);
  h.calls[1].resolve(snapshot('recovered', 'completed')); await flush();
  assert.equal(recovered.length, 1);
  offBroken(); offHealthy();
  assert.equal(h.timers.size, 0);
});

test('a view detached by an earlier callback receives no remaining delivery', async () => {
  const h = observer(); const received = []; const errors = []; let offLater;
  const offFirst = h.subscribe({ next: () => offLater(), error: assert.fail });
  offLater = h.subscribe({ next: value => received.push(value), error: error => errors.push(error) });
  h.calls[0].resolve(snapshot('detach-during-publication', 'completed')); await flush();
  assert.equal(received.length, 0);
  assert.equal(errors.length, 0);
  offFirst();
  assert.equal(h.timers.size, 0);
});

test('a subscriber whose delivery and error callbacks both fail cannot block a healthy view', async () => {
  const h = observer(); const received = [];
  const offBroken = h.subscribe({
    next: () => { throw Error('fixture delivery failed'); },
    error: () => { throw Error('fixture reporting failed'); },
  });
  const offHealthy = h.subscribe({ next: value => received.push(value), error: assert.fail });
  h.calls[0].resolve(snapshot('both-callbacks-failed', 'completed')); await flush();
  assert.equal(received.length, 1);
  assert.equal(h.diagnostics.length, 1);
  assert.equal(h.tick(), 100);
  assert.equal(h.calls[1].cursor, 'both-callbacks-failed');
  offBroken(); offHealthy(); h.calls[1].resolve(snapshot('both-callbacks-failed')); await flush();
  assert.equal(h.timers.size, 0);
});

test('a subscriber attached during publication receives the cached snapshot exactly once', async () => {
  const h = observer(); const received = []; let offLate;
  const offFirst = h.subscribe({
    next: () => {
      offLate = h.subscribe({ next: value => received.push(value), error: assert.fail });
    },
    error: assert.fail,
  });
  h.calls[0].resolve(snapshot('attached-during-delivery', 'completed')); await flush();
  assert.equal(received.length, 1, 'cached replay and the same publication must not deliver twice');
  assert.equal(h.calls.length, 1, 'attaching during delivery cannot start a second IPC wait');
  offLate(); offFirst();
  assert.equal(h.timers.size, 0);
});
