// Run the actual App callback, not a reimplementation of its ordering.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

function callback(bindings, name = "onRewindSession") {
  const file = path.resolve(__dirname, "../src/App.tsx");
  const ast = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let expression;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === name) {
      assert.ok(ts.isCallExpression(node.initializer));
      expression = node.initializer.arguments[0].getText(ast);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(expression, `production ${name} callback must exist`);
  const compiled = ts.transpileModule(`module.exports = (${expression});`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const loaded = { exports: {} };
  new Function("module", ...Object.keys(bindings), compiled)(loaded, ...Object.values(bindings));
  return loaded.exports;
}

function fixture(rewindSession) {
  const original = [{ id: "streaming", content: "existing reply" }];
  const state = { messages: original, detail: null, refreshes: 0, errors: [], requests: 0 };
  const liveTranscripts = { current: new Map([["source", original]]) };
  const activeIdRef = { current: "source" };
  const run = callback({
    activeId: "source", activeIdRef, liveTranscripts,
    rewindSession: async (...args) => { state.requests += 1; return rewindSession(...args); },
    getSessionDetail: async () => ({ conversation: [{ id: "kept", content: "kept" }] }),
    refreshSessions: async () => { state.refreshes += 1; },
    mapConversationToChatMessages: (conversation) => conversation,
    setMessages: (messages) => { state.messages = messages; },
    setDetail: (detail) => { state.detail = detail; },
    message: { error: (text) => state.errors.push(text) },
    console: { error: () => {} },
  });
  return { run, state, original, liveTranscripts, activeIdRef };
}

test("rewind refusal leaves the live transcript and screen unchanged", async () => {
  const f = fixture(async () => { throw new Error("session is busy"); });
  await f.run(4);
  assert.equal(f.state.messages, f.original);
  assert.equal(f.liveTranscripts.current.get("source"), f.original);
  assert.equal(f.state.detail, null);
  assert.equal(f.state.refreshes, 0);
  assert.equal(f.state.errors.length, 1);
  assert.match(f.state.errors[0], /is busy/);
});

test("rewind invalidates cached history only after the backend commits", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const f = fixture(() => gate);
  const pending = f.run(4);
  assert.equal(f.state.messages, f.original);
  assert.equal(f.liveTranscripts.current.get("source"), f.original);
  release();
  await pending;
  assert.equal(f.liveTranscripts.current.has("source"), false);
  assert.deepEqual(f.state.messages, [{ id: "kept", content: "kept" }]);
  assert.equal(f.state.refreshes, 1);
  assert.equal(f.state.errors.length, 0);
});

test("a committed rewind cannot clear the other session selected while awaiting", async () => {
  let release;
  const f = fixture(() => new Promise((resolve) => { release = resolve; }));
  const pending = f.run(4);
  f.activeIdRef.current = "other";
  const other = [{ id: "other", content: "other screen" }];
  f.state.messages = other;
  f.liveTranscripts.current.set("other", other);
  release();
  await pending;
  assert.equal(f.state.messages, other);
  assert.equal(f.liveTranscripts.current.get("other"), other);
  assert.equal(f.liveTranscripts.current.has("source"), false);
});

test("a missing session does not call the backend or mutate the screen", async () => {
  const f = fixture(async () => {});
  await f.run(4, "");
  assert.equal(f.state.requests, 0);
  assert.equal(f.state.messages, f.original);
  assert.equal(f.liveTranscripts.current.get("source"), f.original);
});

function archiveFixture(archiveConversation) {
  const state = { refreshes: 0, changes: [], errors: [], successes: [] };
  const run = callback({
    activeId: "synthetic-active",
    archiveConversation,
    refreshSessions: () => { state.refreshes += 1; },
    setActiveId: (value) => state.changes.push(["active", value]),
    setDetail: (value) => state.changes.push(["detail", value]),
    setMessages: (value) => state.changes.push(["messages", value]),
    setView: (value) => state.changes.push(["view", value]),
    message: { error: (value) => state.errors.push(value), success: (value) => state.successes.push(value) },
  }, "onArchiveSession");
  return { run, state };
}

test("H215 actual archive callback preserves the active screen when AppCore refuses a busy owner", async () => {
  const f = archiveFixture(async () => { throw new Error("synthetic session is busy"); });
  f.run("synthetic-active");
  await new Promise(setImmediate);
  assert.equal(f.state.refreshes, 0);
  assert.deepEqual(f.state.changes, []);
  assert.deepEqual(f.state.successes, []);
  assert.equal(f.state.errors.length, 1);
  assert.match(f.state.errors[0], /session is busy/);
});

test("H215 actual archive callback changes the screen only after a successful durable receipt", async () => {
  let release;
  const f = archiveFixture(() => new Promise((resolve) => { release = resolve; }));
  f.run("synthetic-active");
  assert.deepEqual(f.state.changes, []);
  assert.equal(f.state.refreshes, 0);
  release(true); await new Promise(setImmediate);
  assert.equal(f.state.refreshes, 1);
  assert.deepEqual(f.state.changes, [["active", null], ["detail", null], ["messages", []], ["view", "start"]]);
  assert.equal(f.state.successes.length, 1);
  assert.deepEqual(f.state.errors, []);
});

// Execute the production session-activation effect, including its cleanup.
function planModeEffect(getPlanMode) {
  const file = path.resolve(__dirname, "../src/App.tsx");
  const ast = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let expression;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
      && node.arguments[0]?.getText(ast).includes("getPlanMode(activeId)")) {
      assert.equal(expression, undefined, "one authoritative activation effect");
      expression = node.arguments[0].getText(ast);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(expression);
  const state = { mode: true, errors: [] };
  const bindings = {
    activeId: "synthetic-session", activeIdRef: { current: null },
    view: "chat", activePendingRunKey: null,
    getSessionDetail: async () => ({ context_usage: null }), getPlanMode,
    setDetail: () => {}, setContextUsageByKey: () => {},
    setPlanMode: (value) => { state.mode = value; },
    message: { error: (error) => state.errors.push(error) },
  };
  const compiled = ts.transpileModule(`module.exports = (${expression});`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const module = { exports: {} };
  new Function("module", ...Object.keys(bindings), compiled)(module, ...Object.values(bindings));
  return { state, cleanup: module.exports() };
}

test("plan state is unknown while loading and a failed read never means normal mode", async () => {
  let reject;
  const f = planModeEffect(() => new Promise((_, fail) => { reject = fail; }));
  assert.equal(f.state.mode, null);
  reject(new Error("synthetic recovery failure"));
  await new Promise(setImmediate);
  assert.equal(f.state.mode, null);
  assert.equal(f.state.errors.length, 1);
  assert.match(f.state.errors[0], /\/plan.*\/execute/);
  assert.match(f.state.errors[0], /synthetic recovery failure/);
});

for (const active of [true, false]) {
  test(`plan state applies the authoritative ${active} value`, async () => {
    const f = planModeEffect(async () => active);
    await new Promise(setImmediate);
    assert.equal(f.state.mode, active);
    assert.deepEqual(f.state.errors, []);
  });
}

test("a stale plan-state rejection cannot change another session or publish its error", async () => {
  let reject;
  const f = planModeEffect(() => new Promise((_, fail) => { reject = fail; }));
  f.cleanup();
  f.state.mode = false;
  reject(new Error("previous session"));
  await new Promise(setImmediate);
  assert.equal(f.state.mode, false);
  assert.deepEqual(f.state.errors, []);
});
