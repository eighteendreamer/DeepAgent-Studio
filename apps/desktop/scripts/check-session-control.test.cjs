// Run the actual App callback, not a reimplementation of its ordering.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

function callback(bindings) {
  const file = path.resolve(__dirname, "../src/App.tsx");
  const ast = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let expression;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === "onRewindSession") {
      assert.ok(ts.isCallExpression(node.initializer));
      expression = node.initializer.arguments[0].getText(ast);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(expression, "production rewind callback must exist");
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
