const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");

const model = {
  providerId: "provider", modelName: "model", displayName: "Model", protocol: null,
  contextWindow: 64000, outputLimit: 4096, active: true,
  thinkingLevels: ["off", "low", "medium", "high", "max"], thinkingDistinct: [], thinkingDisable: true,
};

function editor(overrides = {}) {
  const state = [], saved = [];
  let cursor = 0;
  const module = { exports: {} };
  const source = fs.readFileSync(path.resolve(__dirname, "../src/components/settings/HarnessModelEditor.tsx"), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX,
  } }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, require(name) {
    if (name === "react") return { useState(value) {
      const slot = cursor++;
      if (!(slot in state)) state[slot] = typeof value === "function" ? value() : value;
      return [state[slot], next => { state[slot] = typeof next === "function" ? next(state[slot]) : next; }];
    } };
    if (name === "react/jsx-runtime") return {
      jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }),
    };
    if (name.endsWith("harnessModelsApi")) return { HARNESS_PROTOCOLS: ["responses", "chat_completions", "messages"] };
    return new Proxy({}, { get: (_, component) => String(component) });
  } });
  const render = () => {
    cursor = 0;
    return module.exports.HarnessModelEditor({ model, onSave: async input => saved.push(input), onCancel() {}, ...overrides });
  };
  const walk = (predicate, tree) => {
    if (Array.isArray(tree)) return tree.map(child => walk(predicate, child)).find(Boolean);
    if (!tree || typeof tree !== "object") return undefined;
    return predicate(tree) ? tree : walk(predicate, tree.props?.children);
  };
  const find = predicate => walk(predicate, render());
  return {
    saved, render, find,
    change(id, value) { find(node => node.props?.id === id).props.onChange({ target: { value } }); },
    async submit() { await find(node => node.type === "form").props.onSubmit({ preventDefault() {} }); },
  };
}

test("model editing saves actual capacity and neutral thinking fields", async () => {
  const host = editor();
  host.change("model-context", "128000");
  host.change("model-output", "8192");
  await host.submit();
  assert.equal(host.saved.length, 1);
  assert.equal(host.saved[0].contextWindow, 128000);
  assert.equal(host.saved[0].outputLimit, 8192);
  assert.equal(host.saved[0].protocol, null);
  assert.deepEqual(Array.from(host.saved[0].thinkingLevels), model.thinkingLevels);
  assert.equal(host.saved[0].thinkingDisable, true);
});

test("invalid capacity never reaches the settings bridge", async () => {
  for (const value of ["", "0", "-1", "1.2", "NaN", "9007199254740992"]) {
    const host = editor();
    host.change("model-context", value);
    await host.submit();
    assert.equal(host.saved.length, 0, value);
    assert.ok(host.find(node => node.props?.role === "alert"), "validation must be visible");
  }
  const host = editor();
  host.change("model-output", "4294967296");
  await host.submit();
  assert.equal(host.saved.length, 0);
});

test("turning a level off also removes its distinct declaration", async () => {
  const host = editor({ model: { ...model, thinkingDistinct: ["medium"] } });
  host.find(node => node.props?.id === "model-level-medium").props.onCheckedChange(false);
  await host.submit();
  assert.equal(host.saved[0].thinkingLevels.includes("medium"), false);
  assert.deepEqual(Array.from(host.saved[0].thinkingDistinct), []);
});

test("saving failures keep the editor open and show the real error", async () => {
  let cancelled = false;
  const host = editor({ onSave: async () => { throw new Error("storage unavailable"); }, onCancel: () => { cancelled = true; } });
  await host.submit();
  assert.equal(cancelled, false);
  assert.match(host.find(node => node.props?.role === "alert").props.children, /storage unavailable/);
});

test("model save forwards one registry mutation and refreshes capability consumers", async () => {
  const module = { exports: {} }, calls = [], events = [];
  const code = ts.transpileModule(fs.readFileSync(path.resolve(__dirname, "../src/harnessModelsApi.ts"), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports,
    window: { dispatchEvent: event => events.push(event) },
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options.detail; } },
    require: () => ({ invoke: async (command, args) => { calls.push({ command, args }); return model; } }),
  });
  const { active, ...input } = model;
  assert.equal(await module.exports.saveHarnessModel(input), model);
  assert.equal(calls[0].command, "harness_model_save");
  assert.equal(calls[0].args.model, input);
  assert.equal(events[0].type, "deepagent:settings-changed");
  assert.equal(events[0].detail.affectsBalance, false);
});

function settingsHost() {
  const slots = [], effects = [], reads = [], toggles = [];
  let cursor = 0;
  const providers = ["alpha", "beta"].map(id => ({
    id, channel: id, logo: "", modelUrl: "https://example.test/v1",
    protocol: "responses", balanceKind: "none", enabled: true, hasApiKey: true,
  }));
  const deferred = () => {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
  };
  const hooks = {
    memo: component => component,
    useCallback(fn) { cursor++; return fn; },
    useState(value) {
      const i = cursor++;
      if (!(i in slots)) slots[i] = value;
      return [slots[i], next => { slots[i] = typeof next === "function" ? next(slots[i]) : next; }];
    },
    useMemo(fn) { cursor++; return fn(); },
    useRef(value) { const i = cursor++; return slots[i] ??= { current: value }; },
    useEffect(fn, deps) {
      const i = cursor++;
      if (!slots[i] || deps.some((value, j) => value !== slots[i].deps[j])) {
        const previous = slots[i];
        slots[i] = { deps };
        effects.push(() => { previous?.cleanup?.(); slots[i].cleanup = fn(); });
      }
    },
  };
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(path.resolve(__dirname, "../src/components/settings/ModelsSettings.tsx"), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, require(name) {
    if (name === "react") return hooks;
    if (name.endsWith("/VirtualList")) return { VirtualList: props => ({ type: "list", props: { children: props.items.map(props.renderItem) } }) };
    if (name === "react/jsx-runtime") return { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }) };
    if (name.endsWith("harnessModelsApi")) return {
      HARNESS_PROTOCOLS: ["responses"], HARNESS_BALANCE_KINDS: ["none"],
      listHarnessProviders: async () => providers,
      listHarnessModels: providerId => { const read = { providerId, ...deferred() }; reads.push(read); return read.promise; },
      fetchHarnessModels: providerId => { const read = { providerId, ...deferred() }; reads.push(read); return read.promise; },
      setHarnessModelActive: () => { const toggle = deferred(); toggles.push(toggle); return toggle.promise; },
    };
    return new Proxy({}, { get: (_, component) => String(component) });
  } });
  const render = () => {
    cursor = 0;
    const tree = module.exports.ModelsSettings();
    effects.splice(0).forEach(effect => effect());
    return tree;
  };
  const walk = (predicate, tree) => {
    if (Array.isArray(tree)) return tree.map(child => walk(predicate, child)).find(Boolean);
    if (!tree || typeof tree !== "object") return undefined;
    if (typeof tree.type === "function") return walk(predicate, tree.type(tree.props));
    return predicate(tree) ? tree : walk(predicate, tree.props?.children);
  };
  const find = predicate => walk(predicate, render());
  render();
  return {
    reads, toggles, render, find,
    select(channel) { find(node => node.props?.["aria-label"] === `选择供应商 ${channel}`).props.onClick(); render(); },
    active() { return find(node => node.props?.["aria-label"] === "激活 shared-model"); },
    rowName() { return find(node => node.props?.["data-model-name"])?.props.children[0].props.children.props.children; },
    fetch() { return find(node => node.type === "Button" && node.props.children?.includes?.("拉取模型")).props.onClick(); },
  };
}

const flushSettings = () => new Promise(resolve => setImmediate(resolve));
const providerModel = providerId => ({ ...model, providerId, modelName: "shared-model", displayName: providerId, active: false });

test("model navigation has a translated name in both bundled base languages", () => {
  for (const [language, label] of [["zh", "模型"], ["en", "Models"]]) {
    const locale = JSON.parse(fs.readFileSync(path.resolve(__dirname, `../src/locales/${language}.json`), "utf8"));
    assert.equal(locale.settings.tabs.models, label);
  }
});

test("provider switches reuse cached rows and late background replies stay with their provider", async () => {
  const host = settingsHost();
  await flushSettings();
  host.select("alpha");
  host.reads[0].resolve([providerModel("alpha")]);
  await flushSettings();
  assert.ok(host.active());
  host.select("beta");
  assert.equal(host.active(), undefined, "previous provider models must not remain editable");
  host.select("alpha");
  assert.equal(host.reads.length, 2, "each provider is read once, even during rapid switches");
  assert.equal(host.rowName(), "alpha", "the cached provider renders immediately");
  host.reads[1].resolve([providerModel("beta")]);
  await flushSettings();
  assert.equal(host.rowName(), "alpha", "late beta rows must not replace alpha rows");
  host.select("beta");
  assert.equal(host.rowName(), "beta");
  assert.equal(host.reads.length, 2, "returning to beta must not read the database again");
});

test("a late activation reply cannot change a same-named model under another provider", async () => {
  const host = settingsHost();
  await flushSettings();
  host.select("alpha");
  host.reads[0].resolve([providerModel("alpha")]);
  await flushSettings();
  const pending = host.active().props.onCheckedChange(true);
  host.select("beta");
  host.reads[1].resolve([providerModel("beta")]);
  await flushSettings();
  host.toggles[0].resolve(true);
  await pending;
  assert.equal(host.active().props.checked, false);
});

test("a manual refresh finishing after a provider switch cannot replace the current list", async () => {
  const host = settingsHost();
  await flushSettings();
  host.select("alpha");
  host.reads[0].resolve([providerModel("alpha")]);
  await flushSettings();
  const refresh = host.fetch();
  host.select("beta");
  host.reads[1].resolve([providerModel("beta")]);
  await flushSettings();
  host.reads[2].resolve([providerModel("alpha")]);
  await refresh;
  assert.equal(host.rowName(), "beta");
});

test("failed optimistic activation rolls back only its provider and does not expose its error elsewhere", async () => {
  const host = settingsHost();
  await flushSettings(); host.select("alpha");
  host.reads[0].resolve([providerModel("alpha")]);
  host.reads[1].resolve([providerModel("beta")]);
  await flushSettings();
  const pending = host.active().props.onCheckedChange(true);
  assert.equal(host.active().props.checked, true, "feedback must not wait for persistence");
  host.select("beta");
  host.toggles[0].reject(new Error("activation refused"));
  await pending;
  assert.equal(host.active().props.checked, false);
  assert.equal(host.find(node => node.props?.role === "alert"), undefined);
  host.select("alpha");
  assert.equal(host.active().props.checked, false);
  assert.match(host.find(node => node.props?.role === "alert").props.children, /activation refused/);
  const retry = host.active().props.onCheckedChange(true);
  host.toggles[1].resolve(true); await retry;
  assert.equal(host.active().props.checked, true);
  assert.equal(host.find(node => node.props?.role === "alert"), undefined, "a successful retry clears the previous error");
});

test("a stale refresh cannot undo a later activation", async () => {
  const host = settingsHost();
  await flushSettings(); host.select("alpha");
  host.reads[0].resolve([providerModel("alpha")]);
  await flushSettings();
  const refresh = host.fetch();
  const activation = host.active().props.onCheckedChange(true);
  host.toggles[0].resolve(true); await activation;
  host.reads[2].resolve([providerModel("alpha")]); await refresh;
  assert.equal(host.active().props.checked, true);
});
function providerEditor(overrides = {}) {
  const slots = [], saved = [];
  let cursor = 0;
  const module = { exports: {} };
  const source = fs.readFileSync(path.resolve(__dirname, "../src/components/settings/HarnessProviderEditor.tsx"), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX,
  } }).outputText;
  const useState = value => {
    const i = cursor++;
    if (!(i in slots)) slots[i] = typeof value === "function" ? value() : value;
    return [slots[i], next => { slots[i] = typeof next === "function" ? next(slots[i]) : next; }];
  };
  vm.runInNewContext(code, { module, exports: module.exports,
    FileReader: class { readAsDataURL() { this.result = "data:image/png;base64,iVBORw0KGgo="; this.onload(); } },
    Image: class { set src(_) { this.onload(); } },
    require(name) {
      if (name === "react") return { useState, useRef(value) { const i = cursor++; return slots[i] ??= { current: value }; } };
      if (name === "react/jsx-runtime") return { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }) };
      if (name.endsWith("harnessModelsApi")) return { HARNESS_PROTOCOLS: ["responses", "messages", "chat_completions"], HARNESS_BALANCE_KINDS: ["none", "auto", "official"] };
      return new Proxy({}, { get: (_, component) => String(component) });
    },
  });
  const render = () => { cursor = 0; return module.exports.HarnessProviderEditor({ provider: null, onSave: async input => saved.push(input), onCancel() {}, ...overrides }); };
  const walk = (predicate, tree) => Array.isArray(tree) ? tree.map(child => walk(predicate, child)).find(Boolean)
    : !tree || typeof tree !== "object" ? undefined : predicate(tree) ? tree : walk(predicate, tree.props?.children);
  const find = predicate => walk(predicate, render());
  return { saved, find, render,
    change(id, value) { find(node => node.props?.id === id).props.onChange({ target: { value } }); },
    async submit() { await find(node => node.type === "form").props.onSubmit({ preventDefault() {} }); },
    async upload(file) { find(node => node.props?.type === "file").props.onChange({ target: { files: [file], value: "" } }); await flushSettings(); },
  };
}

test("provider form retains stored key and includes the persisted official website", async () => {
  const host = providerEditor({ provider: { id: "layso", channel: "Layso", logo: "/providers/layso.webp", websiteUrl: "https://layso.ai/",
    modelUrl: "https://api.layso.ai/v1", protocol: "responses", balanceKind: "auto", enabled: true, hasApiKey: true } });
  assert.equal(host.find(node => node.props?.id === "harness-key").props.value, "");
  await host.submit();
  assert.equal(host.saved[0].apiKey, null);
  assert.equal(host.saved[0].websiteUrl, "https://layso.ai/");
  assert.equal(host.saved[0].protocol, "responses");
});

test("provider selectors display readable labels before menu items mount", () => {
  const host = providerEditor();
  const protocol = host.find(node => node.type === "Select" && node.props.value === "chat_completions");
  const balance = host.find(node => node.type === "Select" && node.props.value === "auto");
  assert.equal(protocol.props.children[0].props.children.props.placeholder, "Chat Completions");
  assert.equal(balance.props.children[0].props.children.props.placeholder, "自动探测");
});

test("logo upload replaces the draft image and persists it with provider data", async () => {
  const host = providerEditor();
  host.change("harness-channel", " Uploaded ");
  host.change("harness-url", "https://provider.test/v1");
  await host.upload({ type: "image/png", size: 8 });
  await host.submit();
  assert.equal(host.saved[0].logo, "data:image/png;base64,iVBORw0KGgo=");
  assert.equal(host.saved[0].channel, "Uploaded");
});

test("oversized and unsupported logos report errors without replacing the current logo", async () => {
  for (const file of [{ type: "image/png", size: 1_048_577 }, { type: "image/svg+xml", size: 100 }]) {
    const host = providerEditor();
    await host.upload(file);
    assert.ok(host.find(node => node.props?.role === "alert"));
    await host.submit();
    assert.equal(host.saved[0].logo, "");
  }
});

test("provider storage failure stays visible in its dialog", async () => {
  const host = providerEditor({ onSave: async () => { throw new Error("synthetic provider save failure"); } });
  await host.submit();
  assert.match(host.find(node => node.props?.role === "alert").props.children, /synthetic provider save failure/);
});
