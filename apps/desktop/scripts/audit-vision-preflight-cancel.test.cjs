// Opt-in: execute the unmodified App submit/stop callbacks and API client.
// React state is hosted in memory; only Tauri IPC/notifications are controlled.
// No provider, image file, real desktop window, or user database is accessed.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { test } = require("node:test");
const ts = require("typescript");
const { loadTypeScript } = require("./audit-workflow-contract.cjs");

const tick = () => new Promise(setImmediate);
function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

function appCallbacks() {
  const source = readFileSync(path.join(__dirname, "../src/App.tsx"), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const helperNames = ["escapeXmlAttr", "escapeXmlText", "buildSkillContext",
    "normalizeSkillSelections", "buildSkillContexts", "buildMentionContexts",
    "buildPromptWithAttachments", "recognizeImageAttachmentsOnSend"];
  const helpers = new Map();
  const callbacks = new Map();
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && helperNames.includes(node.name?.text)) {
      helpers.set(node.name.text, node.getText(ast));
    }
    if (ts.isVariableDeclaration(node) && ["onSubmit", "onStopRun"].includes(node.name.getText(ast))) {
      assert.equal(node.initializer.expression.getText(ast), "useCallback");
      callbacks.set(node.name.getText(ast), node.initializer.arguments[0].getText(ast));
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.equal(helpers.size, helperNames.length, "load the real attachment/preflight helpers");
  assert.equal(callbacks.size, 2, "load both production callbacks in full");
  return ts.transpileModule([
    ...helpers.values(),
    ...Array.from(callbacks, ([name, code]) => `const ${name} = ${code};`),
    "globalThis.auditCallbacks = { onSubmit, onStopRun };",
  ].join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
}

async function observeStop({ preflight, fresh = false }) {
  const visionEntered = deferred();
  const visionResult = deferred();
  const startEntered = deferred();
  const settled = deferred();
  const handlers = new Map();
  const admitted = new Set();
  const calls = [];
  const notifications = [];
  const timers = new Set();
  let submitted;
  let cancelAccepted;
  const api = loadTypeScript("../src/api.ts", {
    "@tauri-apps/api/core": { invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === "get_vision_settings") {
        return { mode: "system", auto_analyze_pasted_images: true };
      }
      if (command === "vision_recognize_image") {
        visionEntered.resolve();
        return visionResult.promise;
      }
      if (command === "start_chat_v2") {
        submitted = args;
        admitted.add(args.runId);
        startEntered.resolve();
        return { run_id: args.runId, session_id: "audit-session", accepted: true };
      }
      if (command === "cancel_run") {
        // Matches RunCoordinator::request_cancel and the Tauri ACK mapping:
        // an id not yet admitted/registered has no cancellation alias.
        const accepted = admitted.has(args.runId);
        cancelAccepted = accepted;
        return { accepted, state: accepted ? "cancelling" : "not_found" };
      }
      if (command === "get_plan_mode") return false;
      throw new Error(`unexpected IPC: ${command}`);
    } },
    "@tauri-apps/api/event": { listen: async (name, callback) => {
      handlers.set(name, callback);
      return () => { handlers.delete(name); };
    } },
  });
  const { advanceTurnActivity } = loadTypeScript("../src/components/chat-timeline/processDisclosure.ts");
  const context = {
    ...api, advanceTurnActivity, console,
    AbortController,
    activeId: fresh ? null : "audit-session", view: fresh ? "start" : "chat",
    activeIdRef: { current: fresh ? null : "audit-session" },
    activePendingRunKey: null, activePendingRunKeyRef: { current: null },
    activeRunIdsRef: { current: new Map() }, runningSessionIds: new Set(),
    preflightAbortControllersRef: { current: new Map() },
    runPhaseRef: { current: new Map() },
    pendingNewChatSectionRef: { current: null }, projectSelectionRef: { current: null },
    liveTranscripts: { current: new Map() }, messagesRef: { current: [] },
    messages: [], stoppingRunIds: new Set(), approvals: [], contextUsageByKey: new Map(),
    navState: { history: [], index: -1 }, questionsBySession: {},
    STREAM_RENDER_INTERVAL_MS: 16,
    localStorage: { getItem: () => null },
    refreshSessions: () => {},
    runUiTransition: (action) => action(),
    clearQuestionSession: (previous) => previous,
    t: (key, values) => `${key}${values ? JSON.stringify(values) : ""}`,
    message: {
      error: (value) => notifications.push({ kind: "error", value }),
      info: (value) => notifications.push({ kind: "info", value }),
    },
    window: {
      __TAURI_INTERNALS__: {},
      setTimeout: (action, ms) => {
        const timer = setTimeout(() => { timers.delete(timer); action(); }, ms);
        timers.add(timer);
        return timer;
      },
      clearTimeout: (timer) => { clearTimeout(timer); timers.delete(timer); },
    },
  };
  for (const [setter, state] of Object.entries({
    setMessages: "messages", setActiveId: "activeId", setView: "view",
    setChatFrameKey: "chatFrameKey", setActivePendingRunKey: "activePendingRunKey",
    setStoppingRunIds: "stoppingRunIds", setApprovals: "approvals",
    setContextUsageByKey: "contextUsageByKey", setNavState: "navState",
    setPlanMode: "planMode", setRunningSessionIds: "runningSessionIds",
    setQuestionsBySession: "questionsBySession",
  })) {
    context[setter] = (update) => {
      context[state] = typeof update === "function" ? update(context[state]) : update;
      if (setter === "setRunningSessionIds" && context[state].size === 0) settled.resolve();
      if (setter === "setActivePendingRunKey" && context[state] === null) settled.resolve();
    };
  }
  const priorWindow = globalThis.window;
  globalThis.window = context.window;
  let submission;
  try {
    vm.runInNewContext(appCallbacks(), context, { filename: "audit-production-app-callbacks.js" });
    submission = context.auditCallbacks.onSubmit("audit image turn", preflight ? [1, 2].map((index) => ({
      id: `audit-image-${index}`, name: `fixture-${index}.png`, kind: "image", mime: "image/png",
      originalPath: `G:/audit/fixture-${index}.png`, source: "file", size: 100, status: "ready",
    })) : []);
    await (preflight ? visionEntered.promise : startEntered.promise);
    const startsBeforeStop = calls.filter((call) => call.command === "start_chat_v2").length;
    const locallyRunning = context.activeRunIdsRef.current.size === 1;
    const visionCallsBeforeStop = calls.filter((call) => call.command === "vision_recognize_image").length;
    context.auditCallbacks.onStopRun();
    await tick();
    const cancelsBeforeRelease = calls.filter((call) => call.command === "cancel_run").length;
    const stoppingBeforeRelease = context.stoppingRunIds.size > 0;
    visionResult.resolve({ model_id: "audit-vision", text: "AUDIT_RECOGNIZED_IMAGE", raw_json: "{}" });
    await submission;
    if (!preflight) await startEntered.promise;
    await tick();
    const startsAfterStop = calls.filter((call) => call.command === "start_chat_v2").length - startsBeforeStop;
    // Release the actual runChat completion path, including its 100ms delay.
    // This is controlled cleanup, not a claim that the earlier stop worked.
    if (!preflight) handlers.get("chat://event")?.({ payload: {
      run_id: submitted.runId, payload: { type: "run_cancelled" },
    } });
    if (!preflight) handlers.get("session://completed")({ payload: {
      run_id: submitted.runId, session_id: "audit-session", status: "completed", error: null,
    } });
    await settled.promise;
    await tick();
    return {
      fresh, preflight, locallyRunning, startsBeforeStop, startsAfterStop,
      cancelsBeforeRelease, cancelAccepted, stoppingBeforeRelease, visionCallsBeforeStop,
      visionCallsAfterStop: calls.filter((call) => call.command === "vision_recognize_image").length - visionCallsBeforeStop,
      cancelNotFoundVisible: notifications.some((item) => item.kind === "error" && item.value.includes("composer.cancelNotFound")),
      recognizedTextSubmitted: submitted?.prompt.includes("AUDIT_RECOGNIZED_IMAGE") ?? false,
      preflightToolsSubmitted: submitted?.preflightTools.length ?? 0,
      listenersAfterFinish: handlers.size,
    };
  } finally {
    visionResult.resolve({ text: "AUDIT_CLEANUP" });
    for (const timer of timers) clearTimeout(timer);
    if (priorWindow === undefined) delete globalThis.window;
    else globalThis.window = priorWindow;
  }
}

const options = { skip: process.env.RUN_HARNESS_AUDIT !== "1", timeout: 5000 };
test("an admitted text turn still reaches the normal stop path", options, async () => {
  const result = await observeStop({ preflight: false });
  console.log(JSON.stringify(result));
  assert.equal(result.locallyRunning, true);
  assert.equal(result.startsBeforeStop, 1);
  assert.equal(result.cancelsBeforeRelease, 1);
  assert.equal(result.cancelAccepted, true);
  assert.equal(result.stoppingBeforeRelease, true);
  assert.equal(result.cancelNotFoundVisible, false);
  assert.equal(result.startsAfterStop, 0);
  assert.equal(result.listenersAfterFinish, 0);
});

for (const fresh of [false, true]) {
  test(`${fresh ? "new" : "continued"} image turn does not submit after stop during system vision`, options, async () => {
    const result = await observeStop({ preflight: true, fresh });
    console.log(JSON.stringify(result));
    assert.equal(result.locallyRunning, true, "the UI exposes this turn as active");
    assert.equal(result.startsBeforeStop, 0, "stop occurs before backend admission");
    assert.equal(result.cancelsBeforeRelease, 0, "preflight stop is handled by the run-local signal");
    assert.equal(result.cancelAccepted, undefined, "no backend cancel is needed before admission");
    assert.equal(result.listenersAfterFinish, 0, "cleanup the eventual run before the defect assertion");
    assert.equal(result.startsAfterStop, 0, "stopped image preflight must not start a new backend turn");
    assert.equal(result.visionCallsAfterStop, 0, "the next image must not start after stop");
    assert.equal(result.cancelNotFoundVisible, false);
  });
}
