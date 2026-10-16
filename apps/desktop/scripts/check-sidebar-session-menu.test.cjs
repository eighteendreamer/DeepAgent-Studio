const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const ts = require("typescript");

function actions() {
  const source = readFileSync(path.join(__dirname, "../src/App.tsx"), "utf8");
  const handlers = source.slice(source.indexOf("  const onForkSession ="), source.indexOf("  const [isSidebarOpen,"));
  const calls = [];
  const record = (name, result) => async (...args) => { calls.push([name, ...args]); return result; };
  const summary = { id: "target", title: "Target title" };
  const context = {
    useCallback: (fn) => fn, activeId: "current", activeIdRef: { current: "current" },
    detail: { summary: { id: "current" }, timeline: [{ sequence: 999 }] }, sessions: [summary],
    refreshSessions: async () => {}, navigateTo: (...args) => calls.push(["navigate", ...args]),
    message: { success() {}, error() {}, warning() {} }, console,
    getSessionDetail: record("detail", { summary, timeline: [{ sequence: 7 }] }),
    getSessionConversation: record("conversation", []),
    forkSession: record("fork", { new_session_id: "branch" }),
    rewindSession: record("rewind"),
    exportTranscript: record("export", { content: "Target transcript", extension: "json" }),
    saveTranscriptFile: record("save", "saved.json"),
    renameSession: record("rename", summary),
    openSessionInNewWindow: record("window"),
    navigator: { clipboard: { writeText: record("clipboard") } },
    writeClipboardText: record("clipboard"),
    liveTranscripts: { current: new Map() },
    setMessages: (...args) => calls.push(["messages", ...args]),
    setDetail() {}, setSessions() {}, mapConversationToChatMessages: (value) => value,
  };
  const compiled = ts.transpileModule(`${handlers}\nresult = { onForkSession, onRewindSession, onCopySession, onExportSession, onRenameSession, onOpenSessionInNewWindow };`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  vm.runInNewContext(compiled, context);
  return { ...context.result, calls };
}

test("sidebar actions use the explicitly targeted session while another session is active", async () => {
  const a = actions();
  a.onCopySession("target");
  await a.onExportSession("json", "target");
  await a.onRenameSession("New title", "target");
  await a.onForkSession("target");
  a.onOpenSessionInNewWindow("target");
  await a.onRewindSession(4, "target");
  await new Promise(setImmediate);
  assert.ok(a.calls.some(([op, id, seq]) => op === "fork" && id === "target" && seq === 7));
  assert.ok(a.calls.some(([op, id]) => op === "rename" && id === "target"));
  assert.ok(a.calls.some(([op, id]) => op === "window" && id === "target"));
  assert.ok(a.calls.some(([op, id, seq]) => op === "rewind" && id === "target" && seq === 4));
  const exports = a.calls.filter(([op]) => op === "export");
  assert.equal(exports.length, 2);
  assert.ok(exports.every(([, id]) => id === "target"));
  assert.ok(a.calls.some(([op, content]) => op === "clipboard" && content === "Target transcript"));
  assert.equal(a.calls.some(([op]) => op === "messages"), false, "inactive-session rewind must not clear the visible conversation");
});

test("header actions retain their active-session default", async () => {
  const a = actions();
  await a.onForkSession();
  a.onOpenSessionInNewWindow();
  assert.ok(a.calls.some(([op, id, seq]) => op === "fork" && id === "current" && seq === 999));
  assert.ok(a.calls.some(([op, id]) => op === "window" && id === "current"));
});

test("open targets in settings and the menu match the backend allowlist", () => {
  const root = path.join(__dirname, "..");
  const catalog = readFileSync(path.join(root, "src/components/openTargets.ts"), "utf8");
  const targets = [...catalog.matchAll(/title: "([^"]+)"/g)].map((match) => match[1]);
  const rust = readFileSync(path.join(root, "../../crates/deepagent-app-core/src/session_state_service.rs"), "utf8");
  const start = rust.indexOf("pub const OPEN_TARGETS");
  const list = rust.slice(start, rust.indexOf("];", start));
  assert.deepEqual(targets, [...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]));
});

test("deep links use the existing session-window route and encode the session identity", () => {
  const source = readFileSync(path.join(__dirname, "../src/api.ts"), "utf8");
  const start = source.indexOf("export function sessionDeepLink");
  const code = source.slice(start, source.indexOf("\n}", start) + 2);
  const context = { exports: {}, window: { location: { origin: "http://localhost", pathname: "/app", search: "" } } };
  vm.runInNewContext(ts.transpileModule(code, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText, context);
  assert.equal(context.exports.sessionDeepLink("session/中文"), "http://localhost/app#session=session%2F%E4%B8%AD%E6%96%87");
});

function appFunction(name, context) {
  const source = readFileSync(path.join(__dirname, "../src/App.tsx"), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let initializer;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === name) initializer = node.initializer.getText(ast);
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(initializer, `production function ${name} exists`);
  const code = ts.transpileModule(`result = ${initializer};`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  vm.runInNewContext(code, context);
  return context.result;
}

test("section new-chat keeps the current project and ordinary navigation clears the draft scope", () => {
  const ref = { current: null };
  let starts = 0;
  const start = appFunction("onNewSectionChat", { useCallback: fn => fn, pendingNewChatSectionRef: ref,
    onNewChat: () => { starts++; ref.current = null; } });
  start("section-a");
  assert.equal(ref.current, "section-a");
  assert.equal(starts, 1);
  const navigate = appFunction("navigateTo", { useCallback: fn => fn, pendingNewChatSectionRef: ref,
    activeId: "old", view: "start" });
  navigate("old", "start");
  assert.equal(ref.current, null, "even navigation to the same frame discards the section draft");
});

test("a new run assigns its captured section once; continuations do not reassign", async () => {
  const calls = [];
  const errors = [];
  const context = { newSessionSectionId: "section-a", sectionAssignmentStarted: false,
    changeSidebarPreferences: async action => { calls.push(action); }, message: { error: text => errors.push(text) } };
  const assign = appFunction("assignNewSessionSection", context);
  assign(""); assign("new-session"); assign("new-session");
  await new Promise(setImmediate);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].section_id, "section-a");
  assert.equal(calls[0].session_id, "new-session");
  const continuation = appFunction("assignNewSessionSection", { ...context, newSessionSectionId: null, sectionAssignmentStarted: false });
  continuation("existing-session");
  assert.equal(calls.length, 1);
  const failed = appFunction("assignNewSessionSection", { ...context, sectionAssignmentStarted: false,
    changeSidebarPreferences: async () => { throw new Error("removed section"); } });
  failed("new-session-2");
  await new Promise(setImmediate);
  assert.match(errors[0], /removed section/);
});

test("section archive targets its members and refreshes successful results on partial failure", async () => {
  const calls = [];
  const archive = appFunction("onArchiveSection", { useCallback: fn => fn,
    useSidebarPreferences: { getState: () => ({ preferences: { session_sections: { a: "one", b: "two", c: "one" } } }) },
    sessions: [{ id: "a" }, { id: "b" }, { id: "c" }], activeIdRef: { current: "a" },
    archiveConversation: async id => { calls.push(id); if (id === "c") throw new Error("busy c"); return true; },
    refreshSessions: async () => calls.push("refresh"), setDetail() {}, navigateTo: id => calls.push(["navigate", id]),
    message: { success() {} } });
  await assert.rejects(archive("one"), /busy c/);
  assert.ok(calls.includes("a") && calls.includes("c"));
  assert.equal(calls.includes("b"), false);
  assert.ok(calls.includes("refresh"));
  assert.ok(calls.some(call => Array.isArray(call) && call[0] === "navigate" && call[1] === null));
});
