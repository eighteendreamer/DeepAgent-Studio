const assert = require('node:assert/strict');
const { test } = require('node:test');
const { readFileSync } = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function harness(overrides = {}, sinkOverrides = {}) {
  const source = readFileSync(path.join(__dirname, '../src/components/plugins/terminalSessionController.ts'), 'utf8');
  const timers = new Map(); let nextTimer = 1;
  const calls = []; const output = []; const states = []; let gaps = 0;
  let chunk = { cursor: 0, data: [], truncated: false, eof: false };
  let sequence = 0; let wake; let notifyError; let unsubscribed = 0;
  const api = {
    terminalSessionOpen: async (...args) => { calls.push(['open', ...args]); const id = `pty-${++sequence}`; return [{ sessionId: id, runId: args[0], backend: 'ssh', cols: 80, rows: 24 }, { sessionId: id, runId: args[0], leaseId: id, epoch: 1, holder: 'user' }]; },
    terminalSessionRead: async (...args) => { calls.push(['read', ...args]); return chunk; },
    terminalSessionWrite: async (...args) => { calls.push(['write', ...args]); },
    terminalSessionResize: async (...args) => { calls.push(['resize', ...args]); },
    terminalSessionRenew: async (...args) => { calls.push(['renew', ...args]); },
    terminalSessionClose: async (...args) => { calls.push(['close', ...args]); },
    terminalSessionSubscribe: async (session, onWake, onError) => {
      calls.push(['subscribe', session]); wake = onWake; notifyError = onError;
      return () => { unsubscribed++; };
    },
    ...overrides,
  };
  const module = { exports: {} };
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText, {
    exports: module.exports, module, require: () => api, Uint8Array, TextDecoder,
    document: { hidden: false },
    setTimeout: (callback, delay) => { const id = nextTimer++; timers.set(id, { callback, delay }); return id; },
    clearTimeout: id => timers.delete(id),
  });
  const controller = new module.exports.TerminalSessionController({ kind: 'ssh', connectionId: 'fixture' }, 'fixture-run', {
    write: async text => output.push(text),
    state: (state, error) => states.push({ state, error }),
    gap: () => { gaps++; },
    ...sinkOverrides,
  });
  return { controller, calls, output, states, timers, setChunk: value => { chunk = value; }, get gaps() { return gaps; },
    wake: () => wake(), notificationError: error => notifyError(error), get unsubscribed() { return unsubscribed; },
    tick: async () => { const [id, timer] = [...timers].sort((a, b) => a[1].delay - b[1].delay)[0]; timers.delete(id); await timer.callback(); },
  };
}

test('controller uses shared target/lease/cursor and streams split UTF-8 with EOF flush', async () => {
  const h = harness();
  await h.controller.open(80, 24);
  assert.equal(h.calls[0][4].kind, 'ssh');
  const bytes = Buffer.from('中文');
  h.setChunk({ cursor: 2, data: [...bytes.subarray(0, 2)], truncated: false }); await h.tick();
  assert.equal(h.output.join(''), '');
  h.setChunk({ cursor: 6, data: [...bytes.subarray(2)], truncated: false }); await h.tick();
  assert.equal(h.output.join(''), '中文');
  assert.equal(h.calls.filter(call => call[0] === 'read').at(-1)[2], 2);
  h.setChunk({ cursor: 7, data: [0xe4], truncated: false, eof: true }); await h.tick();
  assert.equal(h.output.join(''), '中文�');
  assert.equal(h.states.at(-1).state, 'exited');
  assert.equal(h.timers.size, 0);
  await h.controller.close();
});

test('closing while open is in flight closes only the eventual session and never polls it', async () => {
  let resolve;
  const h = harness({ terminalSessionOpen: () => new Promise(done => { resolve = done; }) });
  const opening = h.controller.open(80, 24);
  await h.controller.close();
  resolve([{ sessionId: 'late', backend: 'ssh' }, { leaseId: 'late-lease' }]); await opening;
  assert.equal(h.calls.length, 1);
  assert.equal(h.calls[0][0], 'close');
  assert.equal(h.calls[0][1].sessionId, 'late');
  assert.equal(h.timers.size, 0);
});

test('late reads cannot render after close', async () => {
  let reply;
  const h = harness({ terminalSessionRead: () => new Promise(done => { reply = done; }) });
  await h.controller.open(80, 24);
  const reading = h.tick();
  await h.controller.close();
  reply({ cursor: 3, data: [111, 108, 100], truncated: false }); await reading;
  assert.equal(h.output.length, 0);
  assert.equal(h.timers.size, 0);
});

test('concurrent close waits for one backend result and a failed close retains the exact generation for retry', async () => {
  const failure = Error('fixture backend cleanup failed');
  const session = { sessionId: 'ssh:fixture:retry', runId: 'fixture-run', backend: 'ssh', connectionGeneration: 42 };
  const lease = { sessionId: session.sessionId, runId: session.runId, leaseId: 'retry-lease', epoch: 7, holder: 'user' };
  let rejectClose;
  const attempts = [];
  const h = harness({
    terminalSessionOpen: async () => [session, lease],
    terminalSessionClose: async (...args) => {
      attempts.push(args);
      if (attempts.length === 1) await new Promise((resolve, reject) => { rejectClose = reject; });
    },
  });
  await h.controller.open(80, 24);
  const first = h.controller.close();
  const second = h.controller.close();
  const outcomes = Promise.allSettled([first, second]);
  await Promise.resolve();
  assert.equal(attempts.length, 1, 'overlapping close requests share one backend cleanup');
  rejectClose(failure);
  const settled = await outcomes;
  assert.deepEqual(settled.map(result => result.status), ['rejected', 'rejected']);
  for (const result of settled) assert.equal(result.reason, failure);
  assert.equal(h.timers.size, 0);
  assert.equal(h.unsubscribed, 1);

  await h.controller.write('must-not-restart');
  await h.controller.resize(100, 40);
  assert.equal(h.calls.filter(call => call[0] === 'write' || call[0] === 'resize').length, 0);
  await h.controller.close();
  assert.equal(attempts.length, 2, 'explicit retry must reach the retained backend owner');
  for (const attempt of attempts) {
    assert.equal(attempt[0], session);
    assert.equal(attempt[1], lease);
  }
  await h.controller.close();
  assert.equal(attempts.length, 2, 'successful cleanup is not sent again');
});

test('a session arriving after disposal keeps its owner when cleanup fails and can be closed again', async () => {
  let resolveOpen;
  const failure = Error('fixture late cleanup failed');
  const session = { sessionId: 'ssh:fixture:late-retry', backend: 'ssh', connectionGeneration: 84 };
  const lease = { sessionId: session.sessionId, leaseId: 'late-retry-lease', epoch: 2 };
  const attempts = [];
  const h = harness({
    terminalSessionOpen: () => new Promise(resolve => { resolveOpen = resolve; }),
    terminalSessionClose: async (...args) => {
      attempts.push(args);
      if (attempts.length === 1) throw failure;
    },
  });
  const opening = h.controller.open(80, 24);
  await h.controller.close();
  resolveOpen([session, lease]);
  await assert.rejects(opening, error => error === failure);
  assert.equal(h.timers.size, 0);
  assert.equal(h.calls.filter(call => call[0] === 'subscribe' || call[0] === 'read').length, 0);
  await h.controller.close();
  assert.equal(attempts.length, 2, 'late owner must survive its first failed cleanup');
  assert.equal(attempts[1][0], session);
  assert.equal(attempts[1][1], lease);
});

test('xterm acknowledgement gates the next read and input/resize are ordered without duplicate sizes', async () => {
  const h = harness(); await h.controller.open(80, 24);
  await Promise.all([h.controller.write('first'), h.controller.write('second'), h.controller.resize(80, 24), h.controller.resize(90, 30), h.controller.resize(90, 30)]);
  assert.equal(h.calls.filter(call => call[0] === 'write').map(call => call[3]).join(','), 'first,second');
  assert.equal(h.calls.filter(call => call[0] === 'resize').length, 1);
  await h.controller.close();
  let parsed, entered;
  const parsing = new Promise(resolve => { entered = resolve; });
  const slow = harness({}, { write: () => new Promise(resolve => { parsed = resolve; entered(); }) });
  await slow.controller.open(80, 24);
  slow.setChunk({ cursor: 1, data: [65], truncated: false });
  const reading = slow.tick();
  await parsing;
  assert.equal([...slow.timers.values()].filter(timer => timer.delay < 60_000).length, 0);
  parsed(); await reading;
  assert.equal([...slow.timers.values()].filter(timer => timer.delay < 60_000).length, 1);
  await slow.controller.close();
});

test('truncated history resets decoder and read errors are visible', async () => {
  const h = harness(); await h.controller.open(80, 24);
  h.setChunk({ cursor: 1, data: [0xe4], truncated: false }); await h.tick();
  h.setChunk({ cursor: 99, data: [65], truncated: true }); await h.tick();
  assert.equal(h.gaps, 1);
  assert.equal(h.output.join(''), 'A');
  await h.controller.close();
  const bad = harness({ terminalSessionRead: async () => { throw Error('fixture read failed'); } });
  await bad.controller.open(80, 24); await bad.tick();
  assert.equal(bad.states.at(-1).state, 'error');
  assert.match(bad.states.at(-1).error, /fixture read failed/);
  assert.equal(bad.timers.size, 0);
  await bad.controller.close();
});

test('subscription precedes immediate drain, and an idle terminal waits for a wakeup rather than polling', async () => {
  const h = harness(); await h.controller.open(80, 24);
  assert.deepEqual(h.calls.map(call => call[0]), ['open', 'subscribe']);
  await h.tick();
  assert.equal(h.calls.filter(call => call[0] === 'read').length, 1, 'drain even without a notification');
  assert.equal([...h.timers.values()].filter(timer => timer.delay < 60_000).length, 0, 'no idle read polling');
  h.setChunk({ cursor: 1, data: [65], truncated: false });
  for (let i = 0; i < 1000; i++) h.wake();
  assert.equal([...h.timers.values()].filter(timer => timer.delay === 0).length, 1);
  await h.tick();
  assert.equal(h.output.join(''), 'A');
  h.setChunk({ cursor: 1, data: [], truncated: false }); await h.tick();
  assert.equal([...h.timers.values()].filter(timer => timer.delay < 60_000).length, 0);
  await h.controller.close();
  assert.equal(h.unsubscribed, 1);
  h.wake();
  assert.equal(h.timers.size, 0);
});

test('wakeups during parser backpressure coalesce into one follow-up read without overlapping reads', async () => {
  let parsed, entered;
  const parsing = new Promise(resolve => { entered = resolve; });
  const h = harness({}, { write: () => new Promise(resolve => { parsed = resolve; entered(); }) });
  await h.controller.open(80, 24);
  h.setChunk({ cursor: 1, data: [65], truncated: false });
  const reading = h.tick(); await parsing;
  for (let i = 0; i < 1000; i++) h.wake();
  assert.equal(h.calls.filter(call => call[0] === 'read').length, 1);
  assert.equal([...h.timers.values()].filter(timer => timer.delay < 60_000).length, 0);
  parsed(); await reading;
  assert.equal([...h.timers.values()].filter(timer => timer.delay === 0).length, 1);
  h.setChunk({ cursor: 1, data: [], truncated: false }); await h.tick();
  assert.equal(h.calls.filter(call => call[0] === 'read').at(-1)[2], 1);
  await h.controller.close();
});

test('late subscription is disposed after close, and notification failures stop the session visibly', async () => {
  let subscribed, entered; let releases = 0;
  const subscribing = new Promise(resolve => { entered = resolve; });
  const h = harness({ terminalSessionSubscribe: () => new Promise(resolve => { subscribed = resolve; entered(); }) });
  const opening = h.controller.open(80, 24);
  await subscribing; await h.controller.close();
  assert.equal(typeof subscribed, 'function');
  subscribed(() => { releases++; }); await opening;
  assert.equal(releases, 1);
  assert.equal(h.calls.filter(call => call[0] === 'read').length, 0);
  assert.equal(h.timers.size, 0);
  const bad = harness(); await bad.controller.open(80, 24);
  bad.notificationError(Error('fixture notification disconnected'));
  assert.equal(bad.states.at(-1).state, 'error');
  assert.match(bad.states.at(-1).error, /notification disconnected/);
  assert.equal(bad.unsubscribed, 1);
  assert.equal(bad.timers.size, 0);
  await bad.controller.close();
});
