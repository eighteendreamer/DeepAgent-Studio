const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");
const root = path.resolve(__dirname, "..");
const tick = () => new Promise(resolve => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function prefetchHost() {
  const module = { exports: {} };
  let now = 100;
  const code = ts.transpileModule(fs.readFileSync(path.join(root, "src/sessionPrefetch.ts"), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, Date: { now: () => now } });
  const calls = [];
  const cache = module.exports.createSessionPrefetch(id => {
    const pending = deferred(); calls.push({ id, ...pending }); return pending.promise;
  });
  return { cache, calls, advance: ms => { now += ms; } };
}
test("startup preload and first navigation share one read, even while in flight", async () => {
  const { cache, calls } = prefetchHost();
  cache.warm("a", 1); cache.warm("a", 1);
  const visible = cache.take("a", 1);
  assert.equal(calls.length, 1);
  const detail = { summary: { id: "a" }, conversation: [] };
  calls[0].resolve(detail);
  assert.equal(await visible, detail);
  cache.take("a", 1);
  assert.equal(calls.length, 2, "consumed previews must not become a stale permanent cache");
});
test("failed preload is discarded; it neither poisons retries nor clears another session", async () => {
  const { cache, calls } = prefetchHost();
  cache.warm("a", 1); calls[0].reject(new Error("offline")); await tick();
  cache.warm("a", 1); assert.equal(calls.length, 2);
  cache.warm("b", 1); calls[1].reject(new Error("late old failure")); await tick();
  const next = cache.take("b", 1);
  assert.equal(calls.length, 3);
  calls[2].resolve({ summary: { id: "b" } });
  assert.equal((await next).summary.id, "b");
});
test("changed revision, different session and expired preloads are re-read", () => {
  for (const kind of ["revision", "identity", "age"]) {
    const { cache, calls, advance } = prefetchHost();
    cache.warm("a", 1);
    if (kind === "age") advance(30_001);
    cache.take(kind === "identity" ? "b" : "a", kind === "revision" ? 2 : 1);
    assert.equal(calls.length, 2, kind);
  }
});
test("startup warming never touches visible state or plan-mode admission", () => {
  const source = fs.readFileSync(path.join(root, "src/App.tsx"), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
      && node.arguments[0]?.getText(ast).includes("sessionPrefetch.warm")) effect = node.arguments[0].getText(ast);
    ts.forEachChild(node, visit);
  }
  visit(ast); assert.ok(effect);
  const calls = [];
  for (const view of ["start", "chat", "settings"]) {
    vm.runInNewContext(`(${effect})()`, { view, activeId: "recent", activeSessionRevision: 7,
      activePendingRunKey: null, sessionPrefetch: { warm: (...args) => calls.push(args) } });
  }
  assert.deepEqual(calls, [["recent", 7]]);
});
test("failed plan display does not leave a warning banner; the active indicator remains", () => {
  const source = fs.readFileSync(path.join(root, "src/components/Composer.tsx"), "utf8");
  assert.doesNotMatch(source, /计划模式状态尚未确认|planMode === null/);
  assert.match(source, /planMode === true/);
});
test("model menu uses the original shared morph timing, not a speed override", () => {
  const menu = fs.readFileSync(path.join(root, "src/components/ModelThinkingSelector.tsx"), "utf8");
  const shell = fs.readFileSync(path.join(root, "src/components/ui/MorphingMenuShell.tsx"), "utf8");
  assert.doesNotMatch(menu + shell, /durationScale|timeline\.timeScale/);
  assert.match(shell, /SHARE_MOTION\.open/);
});
