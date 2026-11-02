// Default H-41/H-134 regression: production TS; synthetic stores/Tauri only.
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { loadTypeScript } = require('./audit-workflow-contract.cjs');
function deferred() {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
async function until(predicate) {
  const deadline = Date.now() + 2000;
  while (!predicate()) {
    assert.ok(Date.now() < deadline, 'workflow fixture did not reach its barrier');
    await new Promise(setImmediate);
  }
}
function fixture({ mode = 'professional', listenGate, startGate, cancelGate, cancelError, cancelAccepted = true } = {}) {
  const previousWindow = globalThis.window;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const calls = [], registrations = [], runs = [];
  const state = {
    nodes: [{ id: 'inspect', data: { kind: mode === 'creative' ? 'video-gen' : 'template-transform', status: 'idle' } }], edges: [],
    updateNodeData(id, patch) {
      this.nodes = this.nodes.map(node => node.id === id ? { ...node, data: { ...node.data, ...patch } } : node);
    },
  };
  const canvas = { mode, workflowApproval: null, setWorkflowApproval(value) { this.workflowApproval = value; } };
  let sequence = 0;
  const executor = loadTypeScript('../src/canvas/workflow/utils/workflowExecutor.ts', {
    '../store/canvasStore': { useCanvasStore: { getState: () => canvas } },
    '../store/professionalStore': { useProfessionalStore: { getState: () => state } },
    '../store/creativeStore': { useCreativeStore: { getState: () => state } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      const call = { command, args }; calls.push(call);
      if (command === 'start_workflow') {
        call.runId = `canvas-run-${++sequence}`;
        assert.ok(registrations.filter(item => item.active).length >= 3, 'subscribe before start');
        if (sequence === 1 && startGate) await startGate.promise;
        return { run_id: call.runId, session_id: null };
      }
      assert.equal(command, 'cancel_run');
      if (args.runId === 'canvas-run-1' && cancelGate) await cancelGate.promise;
      if (args.runId === 'canvas-run-1' && cancelError) throw Error('synthetic cancellation transport failure');
      return { accepted: cancelAccepted, state: cancelAccepted ? 'cancelling' : 'not_found' };
    } },
    '@tauri-apps/api/event': { listen: async (event, callback) => {
      const item = { event, callback, active: true, detached: 0 }; registrations.push(item);
      if (listenGate?.index === registrations.length - 1) await listenGate.promise;
      return () => { item.active = false; item.detached += 1; };
    } },
  });
  const emit = (event, payload) => {
    for (const item of registrations.filter(item => item.active && item.event === event)) item.callback({ payload });
  };
  return { state, canvas, calls, registrations, executor, emit,
    starts: () => calls.filter(call => call.command === 'start_workflow'),
    cancels: () => calls.filter(call => call.command === 'cancel_run').map(call => call.args.runId),
    start(nodeId) { const run = executor.runWorkflow(nodeId); runs.push(run); return run; },
    async ready(count = 1) { await until(() => sequence >= count); await new Promise(setImmediate); },
    async close() {
      listenGate?.resolve(); startGate?.resolve(); cancelGate?.resolve();
      await new Promise(setImmediate);
      const stop = executor.cancelWorkflow();
      await Promise.allSettled([...runs, stop]);
      assert.ok(registrations.every(item => !item.active && item.detached === 1), 'release every listener exactly once');
      if (previousWindow === undefined) delete globalThis.window;
      else globalThis.window = previousWindow;
    },
  };
}
function nodeEvent(runId, status, tag = runId) {
  return { run_id: runId, payload: { type: 'workflow_node', event: {
    node_id: 'inspect', status, elapsed_ms: 1,
    outputs: { videoUrl: `artifact://${tag}`, text: tag, modelId: tag },
    job_id: `job-${tag}`, resume_fingerprint: `fingerprint-${tag}`,
  } } };
}
for (const beforeAck of [false, true]) for (const nodeId of [undefined, 'inspect']) for (const priorStatus of ['idle', 'completed']) {
  test(`H-184 input admission failure remains visible without any node events (before ACK=${beforeAck}, target=${nodeId}, prior=${priorStatus})`, async () => {
    const gate = beforeAck ? deferred() : undefined;
    const f = fixture({ startGate: gate });
    const error = "MissingWorkflowInput: required field customer";
    try {
      f.state.nodes[0].data.status = priorStatus;
      const run = f.start(nodeId); await f.ready();
      f.emit('session://completed', { run_id: 'canvas-run-1', status: 'failed', error });
      gate?.resolve();
      await run;
      assert.equal(f.state.nodes[0].data.status, 'error');
      assert.equal(f.state.nodes[0].data.errorMessage, error);
      assert.equal(f.executor.isWorkflowRunning(), false);
      assert.deepEqual(f.cancels(), []);
      assert.equal(f.registrations.filter(item => item.active).length, 0);
    } finally { await f.close(); }
  });
}
test('foreign completion cannot finish or detach the active canvas run', async () => {
  const f = fixture();
  try {
    f.start(); await f.ready();
    f.emit('chat://event', nodeEvent('canvas-run-1', 'running'));
    const before = structuredClone(f.state.nodes[0].data);
    for (const status of ['completed', 'failed', 'cancelled']) f.emit('session://completed', { run_id: 'foreign', status });
    f.emit('chat://event', nodeEvent('foreign', 'completed'));
    assert.deepEqual(f.state.nodes[0].data, before);
    assert.equal(f.executor.isWorkflowRunning(), true);
    assert.equal(f.registrations.filter(item => item.active).length, 3);
    await f.executor.cancelWorkflow();
    assert.deepEqual(f.cancels(), ['canvas-run-1']);
  } finally { await f.close(); }
});
for (const mode of ['professional', 'creative']) for (const index of [0, 1, 2]) {
  test(`a superseded workflow attempt retires late listener ${index} without overwriting ${mode} state`, async () => {
    const gate = { ...deferred(), index }, f = fixture({ mode, listenGate: gate });
    try {
      const first = f.start(); await until(() => f.registrations.length === index + 1);
      const stale = f.registrations.slice();
      f.start(); await f.ready();
      f.emit('chat://event', nodeEvent('canvas-run-1', 'running', 'current'));
      const before = structuredClone(f.state.nodes[0].data);
      gate.resolve(); await first;
      for (const item of stale) item.callback({ payload: item.event === 'chat://event'
        ? nodeEvent('canvas-run-1', 'completed', 'stale') : { run_id: 'canvas-run-1', status: 'completed' } });
      assert.deepEqual(f.state.nodes[0].data, before, 'retired callbacks cannot change status/output/provider identity');
      assert.equal(f.executor.isWorkflowRunning(), true);
      assert.equal(f.starts().length, 1, 'retired pre-subscription attempt never admitted');
      assert.deepEqual(f.cancels(), []);
      assert.ok(stale.every(item => !item.active && item.detached === 1));
    } finally { await f.close(); }
  });
}
test('replacing an admitted workflow cancels its exact run and isolates stale status, output and identity', async () => {
  const f = fixture({ mode: 'creative' });
  try {
    f.start(); await f.ready(); const stale = f.registrations.slice();
    f.start(); await f.ready(2);
    f.emit('chat://event', nodeEvent('canvas-run-2', 'running', 'current'));
    const before = structuredClone(f.state.nodes[0].data);
    for (const item of stale) item.callback({ payload: item.event === 'chat://event'
      ? nodeEvent('canvas-run-1', 'completed', 'stale') : { run_id: 'canvas-run-1', status: 'completed' } });
    console.log(JSON.stringify({ staleAttemptOverwroteCurrent: f.state.nodes[0].data.status !== 'running', cancelRequestsOnSupersede: f.cancels().length }));
    assert.deepEqual(f.state.nodes[0].data, before);
    assert.deepEqual(f.cancels(), ['canvas-run-1'], 'supersede must notify the kernel, not only abort JS');
    assert.equal(f.executor.isWorkflowRunning(), true);
  } finally { await f.close(); }
});
test('superseded start ACK is retained and cancelled without touching the newer owner', async () => {
  const gate = deferred(), f = fixture({ startGate: gate });
  try {
    const first = f.start(); await f.ready();
    f.start(); await f.ready(2);
    f.emit('chat://event', nodeEvent('canvas-run-2', 'running'));
    const before = structuredClone(f.state.nodes[0].data);
    gate.resolve(); await first;
    assert.deepEqual(f.cancels(), ['canvas-run-1'], 'late ACK must not orphan an admitted run');
    assert.deepEqual(f.state.nodes[0].data, before);
    assert.equal(f.executor.isWorkflowRunning(), true);
  } finally { await f.close(); }
});
test('rapid replacements preserve the predecessor cancellation barrier and issue one request', async () => {
  const gate = deferred(), f = fixture({ cancelGate: gate });
  try {
    f.start(); await f.ready(); const second = f.start();
    await until(() => f.cancels().length > 0);
    f.start(); await new Promise(setImmediate);
    assert.equal(f.starts().length, 1, 'replacement cannot overtake pending cancellation');
    gate.resolve(); await second; await f.ready(2);
    assert.deepEqual(f.cancels(), ['canvas-run-1']);
    assert.equal(f.starts().length, 2, 'intermediate retired owner never starts');
  } finally { await f.close(); }
});
test('stop before the start ACK cancels the late admitted run once', async () => {
  const gate = deferred(), f = fixture({ startGate: gate });
  try {
    const running = f.start(); await f.ready();
    await f.executor.cancelWorkflow('inspect');
    gate.resolve(); await running;
    assert.deepEqual(f.cancels(), ['canvas-run-1']);
    assert.equal(f.executor.isWorkflowRunning(), false);
  } finally { await f.close(); }
});
test('late stop response cannot reset the node owned by a replacement', async () => {
  const gate = deferred(), f = fixture({ cancelGate: gate });
  try {
    f.start(); await f.ready(); const stopping = f.executor.cancelWorkflow('inspect');
    await until(() => f.cancels().length > 0);
    f.start(); gate.resolve(); await stopping; await f.ready(2);
    f.emit('chat://event', nodeEvent('canvas-run-2', 'running'));
    assert.equal(f.state.nodes[0].data.status, 'running');
    assert.deepEqual(f.cancels(), ['canvas-run-1']);
    assert.equal(f.executor.isWorkflowRunning(), true);
  } finally { await f.close(); }
});
test('a superseded start rejection cannot mark the new run failed', async () => {
  const gate = deferred(), f = fixture({ startGate: gate });
  try {
    const first = f.start(); await f.ready();
    f.start(); await f.ready(2);
    f.emit('chat://event', nodeEvent('canvas-run-2', 'running'));
    const before = structuredClone(f.state.nodes[0].data);
    gate.reject(Error('synthetic old start rejected')); await first;
    assert.deepEqual(f.state.nodes[0].data, before);
    assert.deepEqual(f.cancels(), []);
  } finally { await f.close(); }
});
test('replacement does not start after a cancellation transport failure', async () => {
  const f = fixture({ cancelError: true });
  try {
    f.start(); await f.ready(); const replacement = f.start();
    await until(() => f.cancels().length > 0 || f.starts().length > 1);
    await new Promise(setImmediate);
    assert.equal(f.starts().length, 1);
    await replacement;
  } finally { await f.close(); }
});

test('a not-found cancellation neither finishes nor cancels the replacement', async () => {
  const f = fixture({ cancelAccepted: false });
  try {
    f.start(); await f.ready();
    const replacement = f.start(); await f.ready(2);
    f.emit('chat://event', nodeEvent('canvas-run-2', 'running', 'replacement'));
    assert.deepEqual(f.cancels(), ['canvas-run-1']);
    assert.equal(f.executor.isWorkflowRunning(), true);
    assert.equal(f.state.nodes[0].data.status, 'running');
    assert.equal(f.registrations.filter(item => item.active).length, 3);
    f.emit('chat://event', nodeEvent('canvas-run-2', 'completed', 'finished'));
    f.emit('session://completed', { run_id: 'canvas-run-2', status: 'completed' });
    await replacement;
    assert.equal(f.executor.isWorkflowRunning(), false);
    assert.deepEqual(f.cancels(), ['canvas-run-1']);
  } finally { await f.close(); }
});

test('normal completion releases its owner without cancelling the finished run', async () => {
  const f = fixture();
  try {
    const first = f.start(); await f.ready();
    f.emit('chat://event', nodeEvent('canvas-run-1', 'completed', 'normal'));
    f.emit('session://completed', { run_id: 'canvas-run-1', status: 'completed' });
    await first;
    assert.equal(f.executor.isWorkflowRunning(), false);
    assert.equal(f.registrations.filter(item => item.active).length, 0);
    f.start(); await f.ready(2);
    assert.deepEqual(f.cancels(), []);
  } finally { await f.close(); }
});

test('late stop response never resets a newer run that already completed', async () => {
  const gate = deferred(), f = fixture({ cancelGate: gate });
  try {
    f.start(); await f.ready();
    const stopping = f.executor.cancelWorkflow('inspect');
    await until(() => f.cancels().length > 0);
    const newer = f.start();
    gate.resolve(); await f.ready(2);
    f.emit('chat://event', nodeEvent('canvas-run-2', 'completed', 'new-result'));
    f.emit('session://completed', { run_id: 'canvas-run-2', status: 'completed' });
    await newer; await stopping;
    assert.equal(f.state.nodes[0].data.status, 'completed');
    assert.equal(f.state.nodes[0].data.result, 'new-result');
    assert.deepEqual(f.cancels(), ['canvas-run-1']);
  } finally { await f.close(); }
});
