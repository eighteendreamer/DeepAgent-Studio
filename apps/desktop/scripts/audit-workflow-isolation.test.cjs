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
