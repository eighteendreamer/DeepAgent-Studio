const assert = require("node:assert/strict");
const { test } = require("node:test");
const { loadTypeScript } = require("./audit-workflow-contract.cjs");

test("foreign completion cannot finish or detach the active canvas run", {
  skip: process.env.RUN_HARNESS_AUDIT !== "1",
  timeout: 5000,
}, async () => {
  const previousWindow = globalThis.window;
  const handlers = new Map();
  const calls = [];
  const updates = [];
  let ready;
  const listenersReady = new Promise((resolve) => { ready = resolve; });
  const state = {
    nodes: [{ id: "inspect", type: "professional-template-transform", data: { kind: "template-transform", status: "idle" } }],
    edges: [],
    updateNodeData(id, patch) {
      updates.push({ id, patch });
      Object.assign(this.nodes.find((node) => node.id === id).data, patch);
    },
  };
  const executor = loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
    "../store/canvasStore": { useCanvasStore: { getState: () => ({ mode: "professional" }) } },
    "../store/professionalStore": { useProfessionalStore: { getState: () => state } },
    "../store/creativeStore": { useCreativeStore: { getState: () => ({ nodes: [], edges: [] }) } },
    "@tauri-apps/api/core": { invoke: async (command, args) => {
      calls.push({ command, args });
      return command === "start_workflow"
        ? { run_id: "canvas-run-A", session_id: "canvas-session-A", accepted: true }
        : { accepted: true };
    } },
    "@tauri-apps/api/event": { listen: async (event, callback) => {
      handlers.set(event, callback);
      if (event === "session://completed") ready();
      return () => { handlers.delete(event); };
    } },
  });
  const emit = (event, payload) => handlers.get(event)?.({ payload });
  const nodeEvent = (runId, status) => ({
    run_id: runId,
    payload: { type: "workflow_node", event: { node_id: "inspect", status, elapsed_ms: 1 } },
  });
  globalThis.window = { __TAURI_INTERNALS__: {} };
  try {
    let settled = false;
    const running = executor.runWorkflow().then(() => { settled = true; });
    await listenersReady;
    await new Promise(setImmediate);
    emit("chat://event", nodeEvent("canvas-run-A", "running"));
    assert.equal(state.nodes[0].data.status, "running");
    const beforeForeignNode = updates.length;
    emit("chat://event", nodeEvent("chat-run-B", "completed"));
    assert.equal(updates.length, beforeForeignNode, "foreign node events already have a working run filter");
    emit("session://completed", { run_id: "chat-run-B", session_id: "chat-session-B", status: "completed", error: null });
    await new Promise(setImmediate);
    const stoppedByForeignEvent = !executor.isWorkflowRunning();
    const settledByForeignEvent = settled;
    const remainingListeners = handlers.size;
    const beforeOwnTerminal = updates.length;
    emit("chat://event", nodeEvent("canvas-run-A", "completed"));
    const ownTerminalDelivered = updates.length !== beforeOwnTerminal;
    await executor.cancelWorkflow("inspect");
    await running;
    const cancelRequests = calls.filter((call) => call.command === "cancel_run");
    console.log(JSON.stringify({ settledByForeignEvent, stoppedByForeignEvent, remainingListeners, ownTerminalDelivered, cancelRequests: cancelRequests.length }));
    assert.equal(stoppedByForeignEvent, false, "another session cannot clear the active workflow owner");
    assert.equal(remainingListeners, 2);
    assert.equal(ownTerminalDelivered, true);
    assert.deepEqual(cancelRequests.map((call) => call.args.runId), ["canvas-run-A"]);
  } finally {
    await executor.cancelWorkflow("inspect");
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
});

test("a superseded workflow attempt can still overwrite the current node status", {
  skip: process.env.RUN_HARNESS_AUDIT !== "1",
  timeout: 5000,
}, async () => {
  const previousWindow = globalThis.window;
  const listeners = [];
  const calls = [];
  const updates = [];
  const firstRegistrations = [];
  let runNumber = 0;
  let resolveFirstListeners;
  const firstListenersRegistered = new Promise((resolve) => { resolveFirstListeners = resolve; });
  const secondRegistrations = [];
  let resolveSecondListeners;
  const secondListenersRegistered = new Promise((resolve) => { resolveSecondListeners = resolve; });
  const deferred = () => {
    let resolve;
    const promise = new Promise((done) => { resolve = done; });
    return { promise, resolve };
  };
  const firstListenGates = [deferred(), deferred()];
  let firstRun;
  let secondRun;
  const state = {
    nodes: [{ id: "inspect", type: "professional-template-transform", data: { kind: "template-transform", status: "idle" } }],
    edges: [],
    updateNodeData(id, patch) {
      updates.push({ id, patch });
      Object.assign(this.nodes.find((node) => node.id === id).data, patch);
    },
  };
  const executor = loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
    "../store/canvasStore": { useCanvasStore: { getState: () => ({ mode: "professional" }) } },
    "../store/professionalStore": { useProfessionalStore: { getState: () => state } },
    "../store/creativeStore": { useCreativeStore: { getState: () => ({ nodes: [], edges: [] }) } },
    "@tauri-apps/api/core": { invoke: async (command, args) => {
      calls.push({ command, args });
      return command === "start_workflow"
        ? { run_id: `canvas-run-${++runNumber}`, session_id: `canvas-session-${runNumber}`, accepted: true }
        : { accepted: true };
    } },
    "@tauri-apps/api/event": { listen: (event, callback) => {
      const registration = { event, callback, active: true };
      listeners.push(registration);
      const registrationIndex = listeners.length - 1;
      if (registrationIndex < 2) {
        firstRegistrations.push(registration);
        if (firstRegistrations.length === 2) resolveFirstListeners();
        return firstListenGates[registrationIndex].promise.then(() => () => { registration.active = false; });
      }
      secondRegistrations.push(registration);
      if (secondRegistrations.length === 2) resolveSecondListeners();
      return Promise.resolve(() => { registration.active = false; });
    } },
  });
  const emitTo = (registration, payload) => {
    if (registration.active) registration.callback({ payload });
  };
  const nodeEvent = (runId, status) => ({
    run_id: runId,
    payload: { type: "workflow_node", event: { node_id: "inspect", status, elapsed_ms: 1 } },
  });
  globalThis.window = { __TAURI_INTERNALS__: {} };
  try {
    firstRun = executor.runWorkflow();
    await firstListenersRegistered;

    secondRun = executor.runWorkflow();
    await secondListenersRegistered;
    await new Promise(setImmediate);
    const cancelRequestsOnSupersede = calls.filter((call) => call.command === "cancel_run").length;

    emitTo(secondRegistrations.find((item) => item.event === "chat://event"), nodeEvent("canvas-run-2", "running"));
    assert.equal(state.nodes[0].data.status, "running", "the current attempt reached running");

    for (const gate of firstListenGates) gate.resolve();
    await new Promise(setImmediate);

    emitTo(firstRegistrations.find((item) => item.event === "chat://event"), nodeEvent("canvas-run-1", "completed"));
    const staleAttemptOverwroteCurrent = state.nodes[0].data.status === "completed";
    console.log(JSON.stringify({ staleAttemptOverwroteCurrent, cancelRequestsOnSupersede, runId: "canvas-run-1", currentRunId: "canvas-run-2" }));
    assert.equal(staleAttemptOverwroteCurrent, false, "a superseded attempt must not update the current node");

    emitTo(secondRegistrations.find((item) => item.event === "session://completed"), {
      run_id: "canvas-run-2", session_id: "canvas-session-2", status: "completed", error: null,
    });
    await Promise.all([firstRun, secondRun]);
  } finally {
    for (const gate of firstListenGates) gate.resolve();
    secondRegistrations.find((item) => item.event === "session://completed")
      ?.callback({ payload: { run_id: "canvas-run-2", session_id: "canvas-session-2", status: "completed", error: null } });
    await Promise.allSettled([firstRun, secondRun].filter(Boolean));
    for (const registration of listeners) registration.active = false;
    await executor.cancelWorkflow("inspect");
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
});
