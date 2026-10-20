const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");
const root = path.resolve(__dirname, "..");
const timeline = "src/components/chat-timeline/";

function load(relative, importer = require) {
  const filename = path.join(root, relative);
  const module = { exports: {} };
  const source = ts.transpileModule(fs.readFileSync(filename, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  vm.runInNewContext(source, { module, exports: module.exports, require: importer });
  return module.exports;
}

const { ProcessDisclosureStore, advanceTurnActivity } = load(timeline + "processDisclosure.ts");
const { deriveTurnSections } = load(timeline + "deriveTurnSections.ts");
const { chatMessagesToBlocks } = load(timeline + "chatMessageMapper.ts");
const { groupTurns, stableTurnKey } = load(timeline + "groupTurns.ts");

test("answer output folds; later work reopens; every outcome settles and folds", () => {
  for (const outcome of ["success", "failed", "cancelled", "blocked", "max_turns", "budget_exceeded", "context_exhausted"]) {
    const store = new ProcessDisclosureStore();
    let activity = { id: outcome, phase: "working", generation: 0 };
    const read = () => store.read(outcome, activity.phase === "settled" ? "settled" : `${activity.generation}:${activity.phase}`, activity.phase === "working");
    assert.equal(read().open, true);
    store.setDetail(outcome, "reasoning", true);
    activity = advanceTurnActivity(activity, "answering");
    assert.equal(read().open, false);
    assert.equal(Object.keys(read().details).length, 0);
    activity = advanceTurnActivity(activity, "working");
    assert.equal(read().open, true);
    store.setOpen(outcome, true);
    activity = advanceTurnActivity(activity, "settled");
    assert.equal(read().open, false);
    store.setOpen(outcome, true);
    assert.equal(read().open, true);
    assert.equal(advanceTurnActivity(activity, "working"), activity);
    assert.equal(read().open, true);
  }
});

test("manual details survive row remounts and are reset by whole-process collapse", () => {
  const store = new ProcessDisclosureStore();
  store.read("a", "settled", false);
  store.setOpen("a", true);
  for (const id of ["reasoning", "execution", "tool:c1", "dag:d1"]) store.setDetail("a", id, true);
  assert.equal(store.read("a", "settled", false).details["dag:d1"], true);
  store.setOpen("a", false);
  store.setOpen("a", true);
  assert.equal(store.read("a", "settled", false).detailsReset, true);
  assert.equal(Object.keys(store.read("a", "settled", false).details).length, 0);
  assert.equal(store.read("b", "settled", false).open, false);
});

test("store snapshots are stable and subscriptions stop after unmount", () => {
  const store = new ProcessDisclosureStore();
  assert.equal(store.read("a", "settled", false), store.read("a", "settled", false));
  let changes = 0;
  const unlisten = store.subscribe("a", () => changes++);
  store.setOpen("a", true);
  store.setOpen("a", true);
  assert.equal(changes, 1);
  unlisten();
  store.setOpen("a", false);
  assert.equal(changes, 1);
});

test("old subscriptions cannot rewind a later answer or settled phase", () => {
  const store = new ProcessDisclosureStore();
  store.read("a", "1:answering", false);
  assert.equal(store.read("a", "0:working", true).open, false);
  store.read("a", "settled", false);
  store.setOpen("a", true);
  assert.equal(store.read("a", "1:answering", false).open, true);
});

test("actual scroll hook distinguishes virtual-list clamping from user scrolling and preserves anchors", () => {
  const handlers = new Map();
  let observeResize;
  const nodes = [0, 400, 800].map((y, index) => ({ dataset: { processTurn: `turn-${index}` }, y,
    getBoundingClientRect() { return { top: this.y - container.scrollTop, bottom: this.y + 400 - container.scrollTop }; },
  }));
  const container = {
    scrollTop: 0, clientHeight: 500, scrollHeight: 1200,
    getBoundingClientRect: () => ({ top: 0, bottom: 500 }), querySelectorAll: () => nodes,
    addEventListener: (name, handler) => handlers.set(name, handler), removeEventListener() {},
  };
  const react = { useRef: value => ({ current: value }), useCallback: fn => fn, useLayoutEffect: fn => fn() };
  const source = fs.readFileSync(path.join(root, timeline + "useTimelineScroll.ts"), "utf8");
  const module = { exports: {} };
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText, {
    module, exports: module.exports, require: () => react,
    ResizeObserver: class { constructor(callback) { observeResize = callback; } observe() {} disconnect() {} },
  });
  module.exports.useTimelineScroll({ current: container }, { current: {} }, 0, 24);
  assert.equal(container.scrollTop, 700);
  // Browser layout clamps an old position while virtual rows are measured.
  container.scrollHeight = 1500; container.scrollTop = 600;
  handlers.get("scroll")();
  assert.equal(container.scrollTop, 1000);
  handlers.get("wheel")(); container.scrollTop = 450; handlers.get("scroll")(); handlers.get("scrollend")();
  nodes[1].y += 100; container.scrollHeight += 100; observeResize();
  assert.equal(container.scrollTop, 550);
  handlers.get("wheel")(); container.scrollTop = 1100; handlers.get("scroll")(); handlers.get("scrollend")();
  container.scrollHeight += 100; observeResize();
  assert.equal(container.scrollTop, 1200);
});

// Execute the actual App accumulator functions, not a second implementation.
function liveAccumulator() {
  const source = fs.readFileSync(path.join(root, "src/App.tsx"), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const names = [
    "updateTranscript",
    "mutateParts",
    "appendDelta",
    "upsertTool",
    "finalize",
    "finalizeCancelled",
    "onEvent",
    "finishRun",
  ];
  const declarations = new Map();
  function visit(node) {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && names.includes(node.name.text)) {
      declarations.set(node.name.text, node.initializer.getText(ast));
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.equal(declarations.size, names.length);
  const activity = { id: "run-a", phase: "working", generation: 0 };
  const messages = [{ role: "user", content: "question", turn: activity }, { role: "assistant", content: "", parts: [], turn: activity }];
  const liveTranscripts = { current: new Map([["session-a", messages]]) };
  const activeRuns = { current: new Map([["session-a", "run-a"]]) };
  let running = new Set(["session-a"]);
  let stopping = new Set(["run-a"]);
  const context = vm.createContext({
    liveTranscripts, runKey: "session-a", runId: "run-a", runStart: Date.now(), advanceTurnActivity,
    activeRunIdsRef: activeRuns, pendingKey: "pending-a",
    activePendingRunKeyRef: { current: null },
    setRunningSessionIds: update => { running = update(running); },
    setStoppingRunIds: update => { stopping = update(stopping); },
    setActivePendingRunKey: update => update(null), setApprovals: update => update([]),
    scheduleTranscriptRender() {}, console: { log() {} },
    asObjectRecord: value => value, summarize: (_, output) => String(output ?? ""),
  });
  const script = "let modelAttemptBaseParts = null;\n" + names.map(name => `const ${name} = ${declarations.get(name)};`).join("\n") + "\nglobalThis.dispatch = onEvent; globalThis.finish = finishRun;";
  vm.runInContext(ts.transpileModule(script, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, context);
  return {
    dispatch: context.dispatch, finish: context.finish,
    latest: () => liveTranscripts.current.get("session-a")[1],
    user: () => liveTranscripts.current.get("session-a")[0],
    replace: messages => liveTranscripts.current.set("session-a", messages),
    activeRuns, running: () => running,
    stopping: () => stopping,
  };
}

test("real live events fold output, reopen tools/approval/retry, preserve markers and settle failures", () => {
  const live = liveAccumulator();
  live.dispatch({ type: "model_request_started", step: 0 });
  live.dispatch({ type: "reasoning_delta", text: "inspect" });
  live.dispatch({ type: "tool_started", name: "shell", call_id: "c1" });
  live.dispatch({ type: "content_delta", text: "waiting" });
  assert.equal(live.latest().turn.phase, "working");
  live.dispatch({ type: "tool_completed", name: "shell", call_id: "c1", ok: false, output: "retry" });
  live.dispatch({ type: "content_delta", text: "answer" });
  assert.equal(live.latest().turn.phase, "answering");
  live.dispatch({ type: "run_awaiting_approval" });
  assert.equal(live.latest().turn.phase, "working");
  live.dispatch({ type: "model_request_started", step: 1 });
  live.dispatch({ type: "context_compacted", strategy: "summary", summary: "saved", tokens_before: 10, tokens_after: 5 });
  live.dispatch({ type: "content_delta", text: "withdrawn answer" });
  live.dispatch({ type: "model_attempt_reset" });
  assert.equal(live.latest().turn.phase, "working");
  assert.equal(live.latest().parts.some(part => part.text === "withdrawn answer"), false);
  assert.equal(live.latest().parts.some(part => part.kind === "compact_boundary"), true);
  live.dispatch({ type: "run_failed", reason: "network stopped" });
  assert.equal(live.latest().turn.phase, "settled");
  assert.equal(live.user().turn, live.latest().turn);
  assert.match(live.latest().turn.finalAnswer, /network stopped/);
});

test("authoritative completion is displayed even when earlier narration already streamed", () => {
  const live = liveAccumulator();
  live.dispatch({ type: "content_delta", text: "I will inspect" });
  live.dispatch({ type: "tool_started", name: "shell", call_id: "c1" });
  live.dispatch({ type: "tool_completed", name: "shell", call_id: "c1", ok: true });
  live.dispatch({ type: "run_completed", message: "Final result" });
  assert.match(live.latest().content, /Final result/);
  const turn = groupTurns(chatMessagesToBlocks([live.user(), live.latest()]))[0];
  const sections = deriveTurnSections(turn, false);
  assert.equal(sections.assistantContentBlocks[0].text, "Final result");
  assert.equal(sections.processBlocks.some(block => block.text === "I will inspect"), true);
});

test("cancelled tool-only runs settle; successful final text is not duplicated", () => {
  const live = liveAccumulator();
  live.dispatch({ type: "tool_started", name: "shell", call_id: "c1" });
  live.dispatch({ type: "run_cancelled" });
  assert.equal(live.latest().turn.phase, "settled");
  assert.equal(live.latest().parts[0].tool.status, "error");
  const other = liveAccumulator();
  other.dispatch({ type: "content_delta", text: "Final result" });
  other.dispatch({ type: "run_completed", message: "Final result" });
  const sections = deriveTurnSections(groupTurns(chatMessagesToBlocks([other.user(), other.latest()]))[0], false);
  assert.equal(sections.assistantContentBlocks.length, 1);
  assert.equal(sections.processBlocks.length, 0);
});

test("turn keys survive prepending history and phase changes", () => {
  const messages = [{ id: "u1", role: "user", content: "question", turn: { id: "run-a", phase: "settled", generation: 0 } }, { id: "a1", role: "assistant", content: "answer" }];
  const first = groupTurns(chatMessagesToBlocks(messages))[0];
  const next = groupTurns(chatMessagesToBlocks([{ id: "old", role: "user", content: "older" }, ...messages]))[1];
  assert.equal(stableTurnKey(first, 0), stableTurnKey(next, 1));
});

test("an identical earlier narration stays in the process when a tool followed it", () => {
  const source = { role: "assistant", content: "done" };
  const turn = { activity: { id: "run-a", phase: "settled", generation: 0, finalAnswer: "done" }, blocks: [
    { kind: "assistant", id: "narration", text: "done", source },
    { kind: "tool", id: "tool", tool: { call_id: "c1", name: "shell", status: "ok" } },
  ] };
  const sections = deriveTurnSections(turn, false);
  assert.equal(sections.processBlocks[0].id, "narration");
  assert.equal(sections.assistantContentBlocks[0].id, "run-a-final");
});

test("an old run's trailing events cannot settle or overwrite a new run", () => {
  const live = liveAccumulator();
  const activity = { id: "run-b", phase: "working", generation: 0 };
  const messages = [{ role: "user", content: "new question", turn: activity }, { role: "assistant", content: "", parts: [], turn: activity }];
  live.replace(messages);
  live.dispatch({ type: "tool_started", name: "old tool", call_id: "old" });
  live.dispatch({ type: "run_failed", reason: "old failure" });
  assert.equal(live.latest(), messages[1]);
  assert.equal(live.latest().turn.phase, "working");
  assert.equal(live.latest().parts.length, 0);
});

test("old transport cleanup keeps the new run busy and preserves its ownership", () => {
  const live = liveAccumulator();
  live.activeRuns.current.set("session-a", "run-b");
  live.finish();
  assert.equal(live.activeRuns.current.get("session-a"), "run-b");
  assert.equal(live.running().has("session-a"), true);
  assert.equal(live.stopping().has("run-a"), false);
  const completed = liveAccumulator();
  completed.finish();
  assert.equal(completed.activeRuns.current.has("session-a"), false);
  assert.equal(completed.running().has("session-a"), false);
  assert.equal(completed.stopping().has("run-a"), false);
});
