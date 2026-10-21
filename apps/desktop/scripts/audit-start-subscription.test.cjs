// Opt-in failure probes: exercise the production clients with a legal IPC
// ordering. No model, backend process, or network is started.
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { setTimeout: delay } = require("node:timers/promises");
const { loadTypeScript } = require("./audit-workflow-contract.cjs");

function transport(runId, sessionId, startCommand) {
  const handlers = new Map();
  const observations = { starts: 0, earlyTerminalDelivered: false };
  let registrationRequested;
  let releaseRegistration;
  const requested = new Promise((resolve) => { registrationRequested = resolve; });
  const registrationGate = new Promise((resolve) => { releaseRegistration = resolve; });
  const completion = { run_id: runId, session_id: sessionId, status: "completed", error: null };
  function emit(event, payload) {
    const callback = handlers.get(event);
    callback?.({ payload });
    return !!callback;
  }
  return {
    handlers, observations, requested, releaseRegistration, completion, emit,
    imports: {
      "@tauri-apps/api/core": { invoke: async (command) => {
        assert.equal(command, startCommand);
        observations.starts += 1;
        emit("chat://event", {
          run_id: runId,
          payload: { type: "workflow_node", event: {
            node_id: "inspect", status: "completed", elapsed_ms: 1,
            outputs: { text: "AUDIT_OUTPUT" },
          } },
        });
        // A fast backend can emit its terminal before the invoke ACK reaches JS.
        observations.earlyTerminalDelivered = emit("session://completed", completion);
        return { run_id: runId, session_id: sessionId, accepted: true };
      } },
      "@tauri-apps/api/event": { listen: async (event, callback) => {
        if (event === "session://completed") {
          registrationRequested();
          await registrationGate;
        }
        handlers.set(event, callback);
        return () => { handlers.delete(event); };
      } },
    },
  };
}

for (const kind of ["chat", "canvas"]) {
  test(`${kind} completes when the backend terminal precedes subscription readiness`, {
    skip: process.env.RUN_HARNESS_AUDIT !== "1",
    timeout: 5000,
  }, async () => {
    const previousWindow = globalThis.window;
    const bus = transport(`audit-${kind}`, `session-${kind}`, kind === "chat" ? "start_chat_v2" : "start_workflow");
    const state = {
      nodes: [{ id: "inspect", type: "professional-template-transform", data: { kind: "template-transform", status: "idle" } }],
      edges: [],
      updateNodeData(id, patch) { Object.assign(this.nodes.find((node) => node.id === id).data, patch); },
    };
    let settled = false;
    let running;
    try {
      globalThis.window = { __TAURI_INTERNALS__: {} };
      const client = kind === "chat"
        ? loadTypeScript("../src/api.ts", bus.imports)
        : loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
          ...bus.imports,
          "../store/canvasStore": { useCanvasStore: { getState: () => ({ mode: "professional" }) } },
          "../store/professionalStore": { useProfessionalStore: { getState: () => state } },
          "../store/creativeStore": { useCreativeStore: { getState: () => ({ nodes: [], edges: [] }) } },
        });
      running = (kind === "chat"
        ? client.runChat("audit", () => {}, undefined, `session-${kind}`, `audit-${kind}`)
        : client.runWorkflow()).then((result) => { settled = true; return result; });
      await bus.requested;
      await new Promise(setImmediate);
      const startedBeforeSubscriptionReady = bus.observations.starts > 0;
      bus.releaseRegistration();
      // Covers runChat's intentional 100ms terminal dispatch delay.
      await delay(150);
      const pendingAfterReadiness = !settled;
      const nodeCompleted = state.nodes[0].data.status === "completed";
      // A second, deliberately late terminal is a cleanup/control, not a claim
      // that Tauri replays events. Current production only emits once.
      if (!settled) bus.emit("session://completed", bus.completion);
      await running;
      assert.equal(bus.handlers.size, 0, "the control terminal cleans up production listeners");
      console.log(JSON.stringify({ kind, startedBeforeSubscriptionReady,
        earlyTerminalDelivered: bus.observations.earlyTerminalDelivered,
        pendingAfterReadiness, ...(kind === "canvas" ? { nodeCompleted } : {}),
        listenersAfterControl: bus.handlers.size }));
      assert.equal(pendingAfterReadiness, false, "an already completed run must not remain pending after subscription is ready");
      if (kind === "canvas") assert.equal(nodeCompleted, true);
    } finally {
      bus.releaseRegistration();
      if (running && !settled) {
        await new Promise(setImmediate);
        bus.emit("session://completed", bus.completion);
        await running;
      }
      if (previousWindow === undefined) delete globalThis.window;
      else globalThis.window = previousWindow;
    }
  });
}
