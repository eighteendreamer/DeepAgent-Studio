// Opt-in Rust/TypeScript seam probe. Events come from the real Rust publisher;
// the production executor and Zustand store run with an in-memory Tauri bridge.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { loadTypeScript } = require("./audit-workflow-contract.cjs");

const tick = () => new Promise(setImmediate);

async function inspectResume(event, initialData = {}, edit = {}) {
  const creative = loadTypeScript("../src/canvas/workflow/store/creativeStore.ts", {
    "../utils/canvasPromptProfile": { canvasPromptProfilePatch: async () => null },
  }).useCreativeStore;
  creative.setState({
    nodes: [{
      id: "video", type: "creative-video-gen", position: { x: 0, y: 0 },
      data: { kind: "video-gen", status: "idle", videoPrompt: "audit", videoModel: "audit::video", ...initialData },
    }],
    edges: [],
  });
  const handlers = new Map();
  const requests = [];
  let listenersReady;
  const executor = loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
    "../store/canvasStore": { useCanvasStore: { getState: () => ({ mode: "creative" }) } },
    "../store/creativeStore": { useCreativeStore: creative },
    "../store/professionalStore": { useProfessionalStore: { getState: () => ({ nodes: [], edges: [] }) } },
    "@tauri-apps/api/core": { invoke: async (command, args) => {
      if (command === "cancel_run") return { accepted: true };
      assert.equal(command, "start_workflow");
      requests.push(structuredClone(args));
      return { run_id: `audit-video-${requests.length}`, session_id: "audit-session", accepted: true };
    } },
    "@tauri-apps/api/event": { listen: async (name, callback) => {
      handlers.set(name, callback);
      if (name === "session://completed") listenersReady();
      return () => { handlers.delete(name); };
    } },
  });
  let running;
  try {
    let ready = new Promise((resolve) => { listenersReady = resolve; });
    running = executor.runWorkflow("video");
    await ready;
    await tick();
    handlers.get("chat://event")({ payload: { run_id: "audit-video-1", payload: event } });
    const data = creative.getState().nodes[0].data;
    assert.equal(data.status, event.event.status, "the event must reach the actual node store");
    const storedTaskId = data.videoTaskId ?? null;
    if (event.event.status === "completed") {
      handlers.get("session://completed")({ payload: { run_id: "audit-video-1" } });
    } else {
      await executor.cancelWorkflow("video");
    }
    await running;
    creative.getState().updateNodeData("video", edit);
    ready = new Promise((resolve) => { listenersReady = resolve; });
    running = executor.runWorkflow("video");
    await ready;
    await tick();
    const config = requests[1].workflow.definition.nodes.find((node) => node.id === "video").config;
    handlers.get("session://completed")({ payload: { run_id: "audit-video-2" } });
    await running;
    assert.equal(handlers.size, 0, "both runs must release their listeners");
    return {
      receivedTaskId: event.event.job_id, storedTaskId, resumedTaskId: config.videoTaskId ?? null,
      workflow: requests[1].workflow,
    };
  } finally {
    await executor.cancelWorkflow("video");
    await running;
  }
}

async function main() {
  if (process.argv[2] !== "--events-from-stdin") {
    throw new Error("opt-in audit only: run the ignored Rust desktop video progress probe");
  }
  const fixture = JSON.parse(readFileSync(0, "utf8"));
  if (fixture.progress) {
    assert.equal(fixture.progress.type, "workflow_node");
    assert.equal(fixture.progress.event.status, "running");
    assert.equal(fixture.progress.event.updates.jobPhase, "queued");
  }
  assert.equal(fixture.completed.event.status, "completed");
  const previousWindow = globalThis.window;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  try {
    const progress = fixture.progress ? await inspectResume(fixture.progress, fixture.initialData) : undefined;
    const completed = await inspectResume(fixture.completed, fixture.initialData);
    const edited = fixture.edit ? await inspectResume(fixture.completed, fixture.initialData, fixture.edit) : undefined;
    process.stdout.write(JSON.stringify({ progress, completed, edited }));
  } finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
}

const deadline = setTimeout(() => {
  console.error("desktop video progress audit exceeded its local deadline");
  process.exit(1);
}, 5000);
main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
}).finally(() => clearTimeout(deadline));
