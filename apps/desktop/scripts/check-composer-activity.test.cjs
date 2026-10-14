const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");
const root = path.resolve(__dirname, "..");

function compile(relative, context = {}) {
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(path.join(root, relative), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, ...context });
  return module.exports;
}
const { collectComposerActivity, resolveChatRepositoryPath } = compile("src/components/composerActivityData.ts");
const item = (content, status = "pending", active_form = "") => ({ content, status, active_form });
const tool = (id, todos, extra = {}) => ({ call_id: id, name: "todo_write", status: "ok", output: { todos }, ...extra });
const message = (...tools) => ({ role: "assistant", content: "", parts: tools.map(tool => ({ kind: "tool", tool })) });
const labels = result => Array.from(result.todos, todo => [todo.text, todo.status]);

test("existing chats resolve their own project path, never a matching name or stale detail", () => {
  const first = { summary: { id: "a", project: "app" }, project_path: "/first/app" };
  const second = { summary: { id: "b", project: "app" }, project_path: "/second/app" };
  assert.equal(resolveChatRepositoryPath("a", first, "/second/app"), "/first/app");
  assert.equal(resolveChatRepositoryPath("b", first, "/second/app"), null);
  assert.equal(resolveChatRepositoryPath("b", second, "/first/app"), "/second/app");
  assert.equal(resolveChatRepositoryPath("a", null, "/second/app"), null);
  assert.equal(resolveChatRepositoryPath("a", { summary: first.summary }, "/second/app"), null);
  assert.equal(resolveChatRepositoryPath(null, first, "/second/app"), "/second/app");
});

function renderActivityBar(props) {
  const cache = new Map();
  function load(relative) {
    if (cache.has(relative)) return cache.get(relative);
    const exports = compile(relative, { require: specifier => {
      if (!specifier.startsWith(".")) return require(specifier);
      const base = path.join(path.dirname(relative), specifier);
      const dependency = [base + ".tsx", base + ".ts"].find(file => fs.existsSync(path.join(root, file)));
      return load(dependency);
    } });
    cache.set(relative, exports);
    return exports;
  }
  const React = require("react");
  const { renderToStaticMarkup } = require("react-dom/server");
  const { ComposerActivityBar } = load("src/components/ComposerActivityBar.tsx");
  return renderToStaticMarkup(React.createElement(ComposerActivityBar, props));
}

test("empty, loading-only and clean repositories never create a composer activity entry", () => {
  for (const props of [{}, { gitLoading: true }, { gitAvailable: true }, { gitAvailable: true, gitLoading: true }]) {
    assert.equal(renderActivityBar({ todos: [], changes: [], ...props }), "");
  }
  const todoOnly = renderActivityBar({ todos: [{ id: "a", text: "work", status: "pending" }], changes: [], gitAvailable: true });
  assert.match(todoOnly, /待办/);
  assert.doesNotMatch(todoOnly, /个文件变更|读取变更/);
});

test("all unknown Git statistics render as unknown, never a fabricated zero-line change", () => {
  const html = renderActivityBar({
    todos: [], changes: [{ path: "binary.bin", additions: 99, deletions: 99, statsAvailable: false }], gitAvailable: true,
  });
  assert.match(html, /未统计行数/);
  assert.doesNotMatch(html, /新增 0 行|删除 0 行|新增 99 行/);
});

test("live and replayed completed outputs use canonical status and active form", () => {
  const call = tool("a", [item("inspect", "completed"), item("Run tests", "in_progress", "Running tests")], {
    args: JSON.stringify({ todos: [item("stale argument")] }),
  });
  const live = collectComposerActivity([message(call)]);
  const replay = collectComposerActivity(JSON.parse(JSON.stringify([message(call)])));
  assert.deepEqual(labels(live), [["inspect", "completed"], ["Running tests", "in_progress"]]);
  assert.deepEqual(labels(replay), labels(live));
});

test("failed, running, blocked, malformed and unrelated MCP calls cannot replace a snapshot", () => {
  const base = message(tool("a", [item("keep")]));
  for (const candidate of [
    tool("b", [item("bad")], { status: "error" }),
    tool("b", [item("bad")], { status: "running" }),
    tool("b", [item("bad")], { status: "blocked" }),
    tool("b", [item("bad", "unknown")]),
    tool("b", [item("bad")], { name: "mcp__plugin__todo_write" }),
    tool("b", [], { output: { todos: "broken" }, args: JSON.stringify({ todos: [item("bad")] }) }),
  ]) assert.deepEqual(labels(collectComposerActivity([base, message(candidate)])), [["keep", "pending"]]);
});

test("empty writes and read snapshots clear the list; legacy arguments only fill missing output", () => {
  const base = message(tool("a", [item("old")]));
  assert.equal(collectComposerActivity([base, message(tool("b", []))]).todos.length, 0);
  assert.equal(collectComposerActivity([base, message(tool("b", [], { name: "task_list" }))]).todos.length, 0);
  const legacy = tool("old", null, { output: undefined, args: JSON.stringify({ todos: [item("legacy", "done")] }) });
  assert.deepEqual(labels(collectComposerActivity([{ role: "assistant", content: "", tools: [legacy] }])), [["legacy", "completed"]]);
});

test("ordered parts win over duplicate legacy cards; text streaming does not trigger Git reads", () => {
  const good = tool("a", [item("actual")]);
  const msg = { ...message(good), tools: [tool("a", [item("obsolete")])] };
  const result = collectComposerActivity([msg]);
  assert.deepEqual(labels(result), [["actual", "pending"]]);
  assert.equal(collectComposerActivity([{ ...msg, content: "stream" }]).completionKey, result.completionKey);
  assert.notEqual(collectComposerActivity([msg, message(tool("b", []))]).completionKey, result.completionKey);
  assert.equal(collectComposerActivity([]).todos.length, 0);
  assert.deepEqual(labels(collectComposerActivity([message(tool("other-session", [item("other")]))])), [["other", "pending"]]);
});

const tick = () => new Promise(resolve => setImmediate(resolve));
test("session detail and plan mode loaders reject late responses after navigation", async () => {
  const source = fs.readFileSync(path.join(root, "src/App.tsx"), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
      && node.arguments[0]?.getText(ast).includes("getSessionDetail(activeId)")) {
      effect = node.arguments[0].getText(ast);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(effect, "execute the production session detail effect");
  const requests = new Map();
  const activeIdRef = { current: null };
  let detail = null;
  let planMode = false;
  function open(activeId) {
    const detailRequest = deferred();
    const planRequest = deferred();
    requests.set(activeId, { detailRequest, planRequest });
    return vm.runInNewContext(`(${effect})()`, {
      activeId, activeIdRef,
      getSessionDetail: () => detailRequest.promise,
      getPlanMode: () => planRequest.promise,
      setDetail: value => { detail = value; },
      setPlanMode: value => { planMode = value; },
    });
  }
  const closeFirst = open("a");
  closeFirst();
  const closeSecond = open("b");
  const current = { summary: { id: "b" }, project_path: "/second/app" };
  requests.get("b").detailRequest.resolve(current);
  requests.get("b").planRequest.resolve(true);
  await tick();
  requests.get("a").detailRequest.resolve({ summary: { id: "a" }, project_path: "/first/app" });
  requests.get("a").planRequest.resolve(false);
  await tick();
  assert.equal(detail, current);
  assert.equal(planMode, true);
  closeSecond();
});

function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

// Minimal React hook host: execute the actual hook and its cleanup/async code.
function gitHarness() {
  const slots = [];
  const effects = [];
  const storage = new Map();
  const listeners = new Map();
  const calls = [];
  let cursor = 0;
  const same = (a, b) => a && a.length === b.length && a.every((value, i) => Object.is(value, b[i]));
  const react = {
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial;
      return [slots[index], value => { slots[index] = typeof value === "function" ? value(slots[index]) : value; }];
    },
    useRef(value) { const index = cursor++; return slots[index] ??= { current: value }; },
    useCallback(fn, deps) {
      const index = cursor++;
      if (!same(slots[index]?.deps, deps)) slots[index] = { fn, deps };
      return slots[index].fn;
    },
    useEffect(fn, deps) {
      const index = cursor++;
      if (!same(slots[index]?.deps, deps)) effects.push(() => {
        slots[index]?.cleanup?.();
        slots[index] = { deps, cleanup: fn() };
      });
    },
  };
  const events = {
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: (name, fn) => { if (listeners.get(name) === fn) listeners.delete(name); },
  };
  const api = {
    gitProjectStatus: async project => { calls.push(["status", project]); return { project_path: project, is_repo: true }; },
    gitBranchList: async project => { calls.push(["branches", project]); return []; },
    gitChanges: async project => { calls.push(["changes", project]); return { project_path: project, is_repo: true, files: [], additions: 0, deletions: 0 }; },
  };
  const { useGitStatus } = compile("src/hooks/useGitStatus.ts", {
    require: name => name === "react" ? react : api,
    localStorage: { getItem: key => storage.get(key), setItem: (key, value) => storage.set(key, value) },
    window: events, document: { ...events, visibilityState: "visible" },
  });
  return {
    api, storage, calls, listeners,
    render(project, options) {
      cursor = 0;
      const result = useGitStatus(project, api, options);
      effects.splice(0).forEach(effect => effect());
      return result;
    },
    unmount() { slots.forEach(slot => slot?.cleanup?.()); },
  };
}

test("conversation navigation and focus make zero Git requests; cached snapshots stay session-scoped", async () => {
  const host = gitHarness();
  const lazy = id => ({ automatic: false, cacheScope: "chat:" + id });
  host.storage.set("deepagent:git-status:A", JSON.stringify({ version: 2, projectPath: "A", branches: [], changes: { files: [{ path: "unrelated" }] } }));
  host.render("A", lazy("first"));
  await tick();
  let value = host.render("A", lazy("first"));
  assert.equal(value.loading, false);
  assert.equal(value.changes, null);
  assert.equal(host.calls.length, 0);
  assert.equal(host.listeners.size, 0);
  host.render("B", lazy("second"));
  host.render(null, lazy("empty"));
  host.render("A", lazy("first"));
  assert.equal(host.calls.length, 0);
  host.api.gitChanges = async () => { host.calls.push(["changes", "A"]); return { is_repo: true, files: [{ path: "first.txt" }] }; };
  await host.render("A", lazy("first")).refreshChanges();
  const reads = host.calls.length;
  // A different conversation in the same repository cannot display the previous result even for one render.
  assert.equal(host.render("A", lazy("second")).changes, null);
  assert.equal(host.render("A", lazy("second")).changes, null);
  host.render("A", lazy("first"));
  assert.equal(host.render("A", lazy("first")).changes.files[0].path, "first.txt");
  assert.equal(host.calls.length, reads);
  host.unmount();
});

test("production conversation refresh responds to live completions, never history restoration or switching", () => {
  const source = fs.readFileSync(path.join(root, "src/components/ChatView.tsx"), "utf8");
  const ast = ts.createSourceFile("ChatView.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
      && node.arguments[0]?.getText(ast).includes("previousGitActivity.current")) effect = node.getText(ast);
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(effect);
  let dependencies, cleanup;
  const timers = new Map();
  let nextTimer = 0, reads = 0;
  const context = {
    previousGitActivity: { current: null }, refreshGitChanges: () => { reads++; },
    window: { setTimeout: fn => { timers.set(++nextTimer, fn); return nextTimer; }, clearTimeout: id => timers.delete(id) },
    useEffect: (fn, deps) => {
      if (dependencies?.every((value, i) => Object.is(value, deps[i]))) return;
      cleanup?.(); dependencies = deps; cleanup = fn();
    },
  };
  function render(sessionId, gitProjectPath, busy, completionKey) {
    vm.runInNewContext(effect, { ...context, sessionId, gitProjectPath, busy, activity: { completionKey } });
  }
  render("a", null, false, "[]");
  render("a", "A", false, "old");
  render("a", "A", false, "restored history");
  render("b", "B", false, "other history");
  assert.equal(timers.size, 0);
  render("b", "B", true, "other history");
  assert.equal(timers.size, 0);
  render("b", "B", true, "new completion 1");
  render("b", "B", true, "new completion 2");
  assert.equal(timers.size, 1);
  for (const fn of timers.values()) fn(); timers.clear();
  assert.equal(reads, 1);
  render("b", "B", false, "new completion 2");
  assert.equal(timers.size, 1);
  render("c", "C", false, "history");
  assert.equal(timers.size, 0, "switching cancels scheduled work from the previous conversation");
  cleanup?.();
});

test("cached data is revalidated and old-project results cannot overwrite a new project", async () => {
  const host = gitHarness();
  const slow = deferred();
  host.api.gitChanges = project => project === "A" ? slow.promise : Promise.resolve({ project_path: project, is_repo: true, files: [{ path: "B.txt" }], additions: 1, deletions: 0 });
  host.storage.set("deepagent:git-status:A", JSON.stringify({ version: 2, projectPath: "A", branches: [], status: { is_repo: true }, changes: { files: [{ path: "cached" }] } }));
  host.render("A");
  const switched = host.render("B");
  assert.equal(switched.changes, null);
  await tick();
  assert.equal(host.render("B").changes.files[0].path, "B.txt");
  slow.resolve({ project_path: "A", is_repo: true, files: [{ path: "A.txt" }] });
  await tick();
  assert.equal(host.render("B").changes.files[0].path, "B.txt");
  assert.equal(host.calls.some(([kind, project]) => kind === "status" && project === "A"), true);
  host.unmount();
});

test("refreshes coalesce with one trailing read and do not reload branches", async () => {
  const host = gitHarness();
  host.render("A"); await tick();
  const value = host.render("A");
  const slow = deferred();
  let reads = 0;
  host.api.gitChanges = async () => { reads++; if (reads === 1) await slow.promise; return { is_repo: true, files: [], additions: 0, deletions: 0 }; };
  const requests = Array.from({ length: 10 }, () => value.refreshChanges());
  assert.equal(reads, 1);
  slow.resolve(); await Promise.all(requests);
  assert.equal(reads, 2);
  assert.equal(host.calls.filter(([kind]) => kind === "branches").length, 1);
  host.unmount();
});

test("errors stay observable, retry clears them, and no-project removes stale results", async () => {
  const host = gitHarness();
  host.api.gitChanges = async () => { throw new Error("Git query failed"); };
  host.render("A"); await tick();
  let value = host.render("A");
  assert.match(value.error, /Git query failed/);
  host.api.gitChanges = async () => ({ is_repo: false, files: [], additions: 0, deletions: 0 });
  await value.refreshChanges();
  value = host.render("A");
  assert.equal(value.error, null);
  assert.equal(value.changes.is_repo, false);
  const empty = host.render(null);
  assert.equal(empty.changes, null);
  assert.equal(empty.error, null);
  host.unmount();
});

test("returning to the foreground refreshes; unmounted queries cannot publish", async () => {
  const host = gitHarness();
  host.render("A"); await tick();
  const before = host.calls.filter(([kind]) => kind === "changes").length;
  host.listeners.get("focus")(); await tick();
  assert.equal(host.calls.filter(([kind]) => kind === "changes").length, before + 1);
  const slow = deferred();
  host.api.gitChanges = () => slow.promise;
  const pending = host.render("A").refreshChanges();
  host.unmount();
  slow.resolve({ is_repo: true, files: [{ path: "late" }] });
  await pending;
  assert.equal(JSON.parse(host.storage.get("deepagent:git-status:A")).changes.files.length, 0);
  assert.equal(host.listeners.size, 0);
});
