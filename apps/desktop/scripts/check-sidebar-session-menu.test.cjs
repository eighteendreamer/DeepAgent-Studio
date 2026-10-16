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
