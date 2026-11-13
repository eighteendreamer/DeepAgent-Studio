const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const test = require("node:test");
const ts = require("typescript");

function deferred() {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}
const settle = () => new Promise(resolve => setImmediate(resolve));
function fixture() {
  return {
    schema_version: 2, strategy: "fallback", ranking: { method: "system", profile: "balanced" },
    engines: [
      { id: "deepseek", enabled: true, base_url: null },
      { id: "anysearch", enabled: false, base_url: "https://api.anysearch.com" },
      { id: "searxng", enabled: true, base_url: null },
      { id: "duckduckgo", enabled: true, base_url: null },
    ],
    engine_statuses: ["deepseek", "anysearch", "searxng", "duckduckgo"].map(id => ({ id, state: "ready" })),
  };
}

// Exercise the production component's actual handlers with controlled hooks and
// IPC promises. This is not DOM/drag acceptance; that is verified separately.
async function mount({ failRead = false, failKeySave = false, anysearchReady = false } = {}) {
  const cells = [], effects = [], writes = [], engineTests = [], failures = [];
  let cursor = 0, first = true, tree;
  const useState = initial => {
    const index = cursor++;
    if (!(index in cells)) cells[index] = typeof initial === "function" ? initial() : initial;
    return [cells[index], value => { cells[index] = typeof value === "function" ? value(cells[index]) : value; }];
  };
  const hooks = {
    useState, useRef: initial => useState(() => ({ current: initial }))[0],
    useMemo: factory => useState(factory)[0], useCallback: callback => callback,
    useEffect: effect => { if (first) effects.push(effect); },
    useSyncExternalStore: (_subscribe, snapshot) => snapshot(),
  };
  const api = new Proxy({
    getSettings: async () => ({}),
    getWebSearchSettings: async () => {
      if (failRead) throw new Error("synthetic read failure");
      const value = fixture(); value.engines[1].enabled = anysearchReady; return value;
    },
    getAnySearchApiKeyInfo: async () => ({ has_user_key: anysearchReady }),
    setAnySearchApiKey: async () => { if (failKeySave) throw new Error("synthetic credential save failure"); },
    setWebSearchSettings: settings => {
      const gate = deferred(); writes.push({ settings: structuredClone(settings), ...gate }); return gate.promise;
    },
    testWebSearchEngine: async engine => { engineTests.push(engine); return { ok: true, count: 1, error: null }; },
  }, { get: (target, name) => target[name] ?? (async () => null) });
  const components = new Proxy({}, { get: (_, name) => String(name) });
  const filename = path.resolve(__dirname, "../src/components/settings/ConfigSettings.tsx");
  function load(file) {
    const compiled = ts.transpileModule(fs.readFileSync(file, "utf8"), {
      fileName: file, compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true },
    });
    const loaded = new Module(file, module);
    loaded.filename = file; loaded.paths = Module._nodeModulePaths(path.dirname(file));
    const nativeRequire = loaded.require.bind(loaded);
    loaded.require = specifier => {
      if (specifier === "react") return hooks;
      if (specifier === "react-i18next") return { useTranslation: () => ({ t: key => key }) };
      if (specifier === "../../api") return api;
      if (specifier === "../message") return { message: { error(value) { failures.push(value); }, success() {} } };
      if (specifier === "@tauri-apps/api/app") return { getVersion: async () => "0.0.8" };
      if (specifier.startsWith("./")) {
        const child = [".ts", ".tsx"].map(extension => path.resolve(path.dirname(file), `${specifier}${extension}`))
          .find(candidate => fs.existsSync(candidate));
        if (child) return load(child);
      }
      if (specifier.startsWith("../shadcn/") || specifier.startsWith("../ui/")) return components;
      return nativeRequire(specifier);
    };
    loaded._compile(compiled.outputText, file); return loaded.exports;
  }
  const { ConfigSettings } = load(filename);
  function render() { cursor = 0; tree = ConfigSettings(); first = false; return tree; }
  function nodes() {
    const output = [];
    function visit(value) {
      if (Array.isArray(value)) { value.forEach(visit); return; }
      if (!value || typeof value !== "object" || !value.props) return;
      output.push(value); visit(value.props.children);
    }
    visit(tree); return output;
  }
  global.window ??= {};
  render(); effects.forEach(effect => effect()); await settle(); render();
  return {
    writes, engineTests, failures, render,
    row(engine) { return nodes().find(node => node.props["data-search-engine"] === engine)?.props; },
    labelled(label) { const match = nodes().filter(node => node.props["aria-label"] === label); assert.equal(match.length, 1, label); return match[0].props; },
    labelledOrNull(label) { return nodes().find(node => node.props["aria-label"] === label)?.props ?? null; },
    testButton(id) {
      const handle = nodes().find(node => node.props["aria-label"] === `${id}搜索开关`);
      const all = nodes(); const index = all.indexOf(handle);
      return all.slice(0, index).findLast(node => node.type === "Button" && node.props.children === "测试").props;
    },
    view() { return Object.fromEntries(nodes().filter(node => node.props.role === "switch").map(node => [node.props["aria-label"], node.props["aria-checked"]])); },
  };
}

test("search settings serialize successive edits instead of publishing overlapping snapshots", async () => {
  const ui = await mount();
  ui.labelled("AnySearch搜索开关").onClick(); ui.render();
  ui.labelled("DeepSeek Search搜索开关").onClick(); ui.render();
  await settle();
  assert.equal(ui.writes.length, 1, "second edit must wait for the preceding settings commit");
  ui.writes[0].resolve(ui.writes[0].settings); await settle();
  assert.equal(ui.writes.length, 2);
  ui.writes[1].resolve(ui.writes[1].settings); await settle(); ui.render();
  assert.equal(ui.view()["AnySearch搜索开关"], true);
  assert.equal(ui.view()["DeepSeek Search搜索开关"], false);
  assert.equal(ui.failures.length, 0);
});

test("a failed edit is rolled back without replaying it inside a later unrelated edit", async () => {
  const ui = await mount();
  ui.labelled("AnySearch搜索开关").onClick(); ui.render();
  ui.labelled("DeepSeek Search搜索开关").onClick(); ui.render();
  await settle(); ui.writes[0].reject(new Error("synthetic atomic write failure")); await settle();
  assert.equal(ui.writes.length, 2);
  ui.writes[1].resolve(ui.writes[1].settings); await settle(); ui.render();
  assert.equal(ui.view()["AnySearch搜索开关"], false, "failed enable must not be accidentally persisted again");
  assert.equal(ui.view()["DeepSeek Search搜索开关"], false);
  assert.equal(ui.failures.length, 1);
});

test("single-engine test waits for pending search settings publication", async () => {
  const ui = await mount();
  ui.labelled("AnySearch搜索开关").onClick(); ui.render(); await settle();
  const testing = ui.testButton("DuckDuckGo").onClick(); await settle();
  assert.deepEqual(ui.engineTests, [], "do not test an older settings snapshot");
  ui.writes[0].resolve(ui.writes[0].settings); await settle(); await testing;
  assert.deepEqual(ui.engineTests, ["duckduckgo"]);
});

test("failed initial read never allows default-looking settings to overwrite stored choices", async () => {
  const ui = await mount({ failRead: true });
  const properties = ui.labelledOrNull("AnySearch搜索开关");
  assert.ok(properties === null || properties.disabled === true);
  assert.equal(ui.writes.length, 0);
});

test("failed AnySearch key save never runs the test with the old credential", async () => {
  const ui = await mount({ failKeySave: true, anysearchReady: true });
  ui.labelled("AnySearch API Key").onChange({ target: { value: "synthetic-new-credential" } }); ui.render();
  await ui.testButton("AnySearch").onClick();
  await settle();
  assert.deepEqual(ui.engineTests, []);
  assert.equal(ui.failures.length, 1);
});

test("double activation before React renders admits only one engine test", async () => {
  const ui = await mount();
  const handler = ui.testButton("DuckDuckGo").onClick;
  await Promise.all([handler(), handler()]);
  assert.deepEqual(ui.engineTests, ["duckduckgo"]);
});

test("drop and keyboard reorder share one persisted engine order without changing switches", async () => {
  const ui = await mount();
  let accepted = false;
  ui.row("deepseek").onDrop({ preventDefault() { accepted = true; }, dataTransfer: { getData: () => "duckduckgo" } });
  await settle(); assert.equal(accepted, true);
  assert.deepEqual(ui.writes[0].settings.engines.map(engine => engine.id), ["duckduckgo", "deepseek", "anysearch", "searxng"]);
  ui.writes[0].resolve(ui.writes[0].settings); await settle(); ui.render();
  ui.labelled("拖动调整DuckDuckGo优先级").onKeyDown({ key: "ArrowDown", preventDefault() {} });
  await settle();
  assert.deepEqual(ui.writes[1].settings.engines.map(engine => engine.id), ["deepseek", "duckduckgo", "anysearch", "searxng"]);
  assert.equal(ui.writes[1].settings.engines.find(engine => engine.id === "anysearch").enabled, false);
  ui.writes[1].resolve(ui.writes[1].settings); await settle();
});
