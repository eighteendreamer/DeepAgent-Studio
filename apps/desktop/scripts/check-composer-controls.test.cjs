const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");
const root = path.resolve(__dirname, "..");
const tick = () => new Promise(resolve => setImmediate(resolve));

function sessionLoader() {
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
  assert.ok(effect);
  return (overrides = {}) => {
    const calls = [], errors = [];
    let planMode;
    const cleanup = vm.runInNewContext(`(${effect})()`, {
      activeId: "legacy", view: "start", activePendingRunKey: null,
      activeIdRef: { current: null },
      getSessionDetail: async id => { calls.push(["detail", id]); return {}; },
      getPlanMode: async id => { calls.push(["plan", id]); throw new Error("unknown legacy mode"); },
      setDetail() {}, setContextUsageByKey() {},
      setPlanMode(value) { planMode = value; },
      message: { error(error) { errors.push(error); } },
      ...overrides,
    });
    return { calls, errors, cleanup, mode: () => planMode };
  };
}

test("home and non-chat pages do not read the remembered legacy session's mode", async () => {
  for (const view of ["start", "settings", "skills"]) {
    const host = sessionLoader()({ view });
    await tick();
    assert.deepEqual(host.calls, []);
    assert.deepEqual(host.errors, []);
    assert.equal(host.mode(), false);
  }
});

test("a new pending chat never loads the previous session or resets its requested plan mode", async () => {
  for (const activeId of ["legacy", null]) {
    let mode = true;
    const host = sessionLoader()({ activeId, view: "chat", activePendingRunKey: "__pending__:new", setPlanMode: v => { mode = v; } });
    await tick();
    assert.deepEqual(host.calls, []);
    assert.equal(mode, true);
  }
});

test("opening a legacy chat still exposes unknown mode; navigating away suppresses late errors", async () => {
  const load = sessionLoader();
  const opened = load({ view: "chat" });
  await tick();
  assert.equal(opened.mode(), null);
  assert.equal(opened.errors.length, 1);
  assert.match(opened.errors[0], /unknown legacy mode/);
  const left = load({ view: "chat" });
  left.cleanup();
  await tick();
  assert.deepEqual(left.errors, []);
});

test("the four display levels are none, low, medium, high without changing provider values", () => {
  const source = fs.readFileSync(path.join(root, "src/components/Composer.tsx"), "utf8");
  const text = source.match(/const THINKING_OPTIONS = (\[[\s\S]*?\]) as const;/)?.[1];
  assert.ok(text);
  const options = vm.runInNewContext(text, { Feather: 0, Lightbulb: 0, Brain: 0, Sparkles: 0 });
  const zh = JSON.parse(fs.readFileSync(path.join(root, "src/locales/zh.json"), "utf8"));
  assert.deepEqual(Array.from(options, o => o.id), ["none", "low", "high", "max"]);
  assert.deepEqual(Array.from(options, o => zh.composer[o.label.split(".")[1]]), ["无", "低", "中", "高"]);
});

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function settingsHost() {
  const slots = [], effects = [], listeners = new Set(), reads = [], writes = [], errors = [];
  let cursor = 0;
  const hooks = {
    useRef(value) { const i = cursor++; return slots[i] ??= { current: value }; },
    useState(value) {
      const i = cursor++;
      if (!(i in slots)) slots[i] = value;
      return [slots[i], next => { slots[i] = typeof next === "function" ? next(slots[i]) : next; }];
    },
    useEffect(fn) { const i = cursor++; if (!(i in slots)) { slots[i] = {}; effects.push(() => { slots[i].cleanup = fn(); }); } },
  };
  const api = {
    SETTINGS_CHANGED_EVENT: "settings",
    getSettings() { const d = deferred(); reads.push(d); return d.promise; },
    setChatModel(value) { const d = deferred(); writes.push({ ...d, field: "chat_model", value }); return d.promise; },
    setReasoningEffort(value) { const d = deferred(); writes.push({ ...d, field: "reasoning_effort", value }); return d.promise; },
  };
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(path.join(root, "src/hooks/useComposerModelSettings.ts"), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports,
    window: { addEventListener: (_, fn) => listeners.add(fn), removeEventListener: (_, fn) => listeners.delete(fn) },
    require(name) {
      if (name === "react") return hooks;
      if (name === "react-i18next") return { useTranslation: () => ({ t: key => key }) };
      if (name === "../api") return api;
      if (name === "../components/message") return { message: { success() {}, error: e => errors.push(e) } };
      throw new Error(`Unexpected import: ${name}`);
    },
  });
  return {
    reads, writes, errors,
    render() { cursor = 0; const result = module.exports.useComposerModelSettings(); effects.splice(0).forEach(fn => fn()); return result; },
    changed() { listeners.forEach(fn => fn()); },
    unmount() { slots.forEach(slot => slot?.cleanup?.()); },
  };
}
const settings = { available_models: ["flash", "pro"], chat_model: "flash", reasoning_effort: "high", model_capabilities: [] };

test("rapid choices are optimistic, serialized and coalesced; stale saves never rewind the UI", async () => {
  const host = settingsHost();
  host.render(); host.reads[0].resolve(settings); await tick();
  host.render().chooseThinking("low");
  host.render().chooseThinking("none");
  host.render().chooseThinking("max");
  assert.equal(host.render().selectedThinking, "max");
  assert.equal(host.writes.length, 1);
  host.changed(); // the API emits before returning its receipt
  assert.equal(host.reads.length, 1);
  host.writes[0].resolve({ ...settings, reasoning_effort: "low" }); await tick();
  assert.equal(host.render().selectedThinking, "max");
  assert.deepEqual(host.writes.map(w => w.value), ["low", "max"]);
  host.writes[1].resolve({ ...settings, reasoning_effort: "max" }); await tick();
  assert.equal(host.render().selectedThinking, "max");
  assert.equal(host.reads.length, 2, "only one reconciliation after draining");
  host.unmount();
});

test("a failed save rolls back to the saved value, not an earlier optimistic choice", async () => {
  const host = settingsHost();
  host.render(); host.reads[0].resolve(settings); await tick();
  host.render().chooseThinking("low");
  host.render().chooseThinking("max");
  host.writes[0].reject(new Error("first write failed")); await tick();
  assert.equal(host.render().selectedThinking, "max");
  host.writes[1].reject(new Error("second write failed")); await tick();
  assert.equal(host.render().selectedThinking, "high");
  assert.equal(host.errors.length, 2);
  host.unmount();
});

test("old reads cannot replace newer selections; external updates still refresh when idle", async () => {
  const host = settingsHost();
  host.render(); host.reads[0].resolve(settings); await tick();
  host.changed();
  host.render().chooseThinking("max");
  host.writes[0].resolve({ ...settings, reasoning_effort: "max" }); await tick();
  host.reads[1].resolve(settings); await tick();
  assert.equal(host.render().selectedThinking, "max");
  host.reads[2].resolve({ ...settings, reasoning_effort: "max" }); await tick();
  host.changed(); host.reads[3].resolve({ ...settings, chat_model: "pro", reasoning_effort: "low" }); await tick();
  assert.equal(host.render().selectedModel, "pro");
  assert.equal(host.render().selectedThinking, "low");
  host.unmount();
});

test("model and effort writes share a queue and navigation does not discard an accepted choice", async () => {
  const host = settingsHost();
  host.render(); host.reads[0].resolve(settings); await tick();
  host.render().chooseModel("pro");
  host.render().chooseThinking("max");
  assert.equal(host.writes.length, 1);
  assert.equal(host.render().selectedModel, "pro");
  assert.equal(host.render().selectedThinking, "max");
  host.unmount();
  host.writes[0].resolve({ ...settings, chat_model: "pro" }); await tick();
  assert.equal(host.writes[1].value, "max");
  host.writes[1].resolve({ ...settings, chat_model: "pro", reasoning_effort: "max" }); await tick();
  assert.equal(host.reads.length, 1, "no reads after unmount");
});
