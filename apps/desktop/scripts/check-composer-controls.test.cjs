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
      && node.arguments[0]?.getText(ast).includes("sessionPrefetch.take(activeId")) {
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
      activeSessionRevision: 1,
      sessionPrefetch: { take: async id => { calls.push(["detail", id]); return {}; } },
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

test("failed auxiliary state is discarded without treating unknown mode as normal", async () => {
  const load = sessionLoader();
  const opened = load({ view: "chat" });
  await tick();
  assert.equal(opened.mode(), null);
  assert.equal(opened.errors.length, 0);
  const left = load({ view: "chat" });
  left.cleanup();
  await tick();
  assert.deepEqual(left.errors, []);
});

test("h029 the five neutral display levels follow Tianshu controls", () => {
  const source = fs.readFileSync(path.join(root, "src/components/Composer.tsx"), "utf8");
  const text = source.match(/const THINKING_OPTIONS = (\[[\s\S]*?\]) as const/)?.[1];
  assert.ok(text);
  const options = vm.runInNewContext(text, { Feather: 0, Zap: 5, Lightbulb: 1, Brain: 2, BrainCircuit: 3, Flame: 6, Sparkles: 4 });
  const zh = JSON.parse(fs.readFileSync(path.join(root, "src/locales/zh.json"), "utf8"));
  assert.deepEqual(Array.from(options, o => o.id), ["off", "low", "medium", "high", "max"]);
  assert.deepEqual(Array.from(options, o => zh.composer[o.label.split(".")[1]]), ["关闭", "低", "中", "高", "最高"]);
  assert.equal(new Set(Array.from(options, o => o.icon)).size, 5, "each level needs a distinct icon");
});

test("thinking suggestions emit canonical backend efforts while displaying off", () => {
  const source = fs.readFileSync(path.join(root, "src/components/Composer.tsx"), "utf8");
  const ast = ts.createSourceFile("Composer.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
      && node.arguments[0]?.getText(ast).includes('commandName === "thinking"')) {
      effect = node.arguments[0].getText(ast);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(effect, "exercise the composer's actual suggestion effect");
  let suggestions;
  const script = ts.transpileModule(`(${effect})()`, {
    compilerOptions: { target: ts.ScriptTarget.ES2020 },
  }).outputText;
  for (const defaultThinking of ["default", "high"]) {
    vm.runInNewContext(script, {
      draftValue: "/thinking ",
      activeModelOptions: [{ id: "chosen", defaultThinking, thinkingLevels: ["off", "minimal", "low", "medium", "high", "xhigh", "max"] }],
      selectedModel: "chosen",
      setSlashResults(value) { suggestions = value; },
      setSlashSelected() {},
    });
    const expected = ["off", "low", "medium", "high", "max"];
    assert.deepEqual(Array.from(suggestions, option => option.title), expected);
    assert.deepEqual(Array.from(suggestions, option => option.insertText.trim()),
      expected.map(level => `/thinking ${level === "off" ? "none" : level}`));
  }
});

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function settingsHost(apiOverrides = {}) {
  const slots = [], effects = [], listeners = new Set(), reads = [], writes = [], errors = [];
  let savedSelection = { ...activeModel("flash", "deepseek"), reasoningEffort: "high" };
  let cursor = 0;
  const hooks = {
    useRef(value) { const i = cursor++; return slots[i] ??= { current: value }; },
    useMemo(factory, dependencies) {
      const i = cursor++;
      if (!(i in slots) || dependencies.some((value, index) => !Object.is(value, slots[i].dependencies[index]))) {
        slots[i] = { dependencies, value: factory() };
      }
      return slots[i].value;
    },
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
    listActiveHarnessModels: async () => [activeModel("flash", "deepseek"), activeModel("pro", "deepseek")],
    getHarnessModelSelection: async () => savedSelection,
    selectHarnessModel(providerId, modelName, affectsBalance, reasoningEffort = null) {
      const d = deferred(); writes.push({ ...d, field: reasoningEffort === null ? "chat_model" : "reasoning_effort",
        value: reasoningEffort ?? modelName, modelName, reasoningEffort, providerId });
      return d.promise.then(() => (savedSelection = { ...activeModel(modelName, providerId), reasoningEffort }));
    },
    setReasoningEffort(value) { const d = deferred(); writes.push({ ...d, field: "reasoning_effort", value }); return d.promise; },
    ...apiOverrides,
  };
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(path.join(root, "src/hooks/useComposerModelSettings.ts"), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports,
    window: { addEventListener: (_, fn) => listeners.add(fn), removeEventListener: (_, fn) => listeners.delete(fn) },
    console: { error() {} },
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
    changed(detail) { listeners.forEach(fn => fn({ detail })); },
    unmount() { slots.forEach(slot => slot?.cleanup?.()); },
  };
}
const settings = { configured: true, reasoning_effort: "high" };

function activeModel(modelName, providerId = "provider") {
  return {
    providerId, modelName, displayName: `Saved ${modelName}`, providerChannel: "Provider",
    contextWindow: 128000, thinkingLevels: ["off", "minimal", "low", "medium", "high", "xhigh", "max"],
    thinkingDistinct: [], thinkingDisable: true,
    reasoningEffort: null, defaultReasoningEffort: null,
  };
}
const modelId = model => JSON.stringify([model.providerId, model.modelName]);

for (const count of [2, 200, 2000]) {
  test(`saved model restores independently of a delayed ${count}-model catalog, without writes`, async () => {
    const catalog = deferred(), selection = deferred();
    const selected = activeModel(`model-${count - 1}`);
    let catalogReads = 0, selectionReads = 0, modelWrites = 0;
    const host = settingsHost({
      listActiveHarnessModels() { catalogReads++; return catalog.promise; },
      getHarnessModelSelection() { selectionReads++; return selection.promise; },
      selectHarnessModel() { modelWrites++; throw new Error("restoring must never select"); },
    });
    host.render();
    host.reads[0].resolve(settings);
    await tick();
    assert.equal(host.render().selectedModel, "", "legacy settings must not be shown as the saved selection");
    selection.resolve(selected);
    await tick();
    const restored = host.render();
    assert.equal(restored.selectedModel, modelId(selected));
    assert.equal(restored.modelOptions.find(option => option.id === restored.selectedModel).label, selected.displayName);
    assert.equal(restored.switching, false);
    catalog.resolve(Array.from({ length: count }, (_, i) => activeModel(`model-${i}`)));
    await tick();
    assert.equal(host.render().selectedModel, modelId(selected));
    assert.equal(host.render().modelOptions.length, count);
    assert.deepEqual([host.reads.length, catalogReads, selectionReads, modelWrites, host.writes.length], [1, 1, 1, 0, 0]);
    host.unmount();
  });
}

test("saved selection and its label remain usable when settings or the catalog cannot load", async () => {
  const catalog = deferred();
  const selected = activeModel("chosen");
  const host = settingsHost({
    listActiveHarnessModels: () => catalog.promise,
    getHarnessModelSelection: async () => selected,
  });
  host.render();
  await tick();
  assert.equal(host.render().selectedModel, modelId(selected), "settings must not block selection restoration");
  catalog.reject(new Error("catalog unavailable"));
  host.reads[0].reject(new Error("settings unavailable"));
  await tick();
  assert.equal(host.render().selectedModel, modelId(selected));
  assert.equal(host.render().modelOptions[0].label, selected.displayName);
  assert.equal(host.errors.length, 1, "a failed load must be reported once, without a misleading switch error");
  assert.deepEqual(host.writes, []);
  host.unmount();
});

test("a failed saved-selection read never falls back to the legacy model", async () => {
  const legacy = activeModel("flash", "deepseek");
  const host = settingsHost({
    listActiveHarnessModels: async () => [legacy],
    getHarnessModelSelection: async () => { throw new Error("selection unavailable"); },
  });
  host.render(); host.reads[0].resolve(settings); await tick();
  assert.equal(host.render().selectedModel, "");
  assert.equal(host.errors.length, 1);
  assert.deepEqual(host.writes, []);
  host.unmount();
});

test("a loaded catalog never supplies a provisional model before the saved selection", async () => {
  const selection = deferred(), selected = activeModel("chosen");
  const host = settingsHost({
    listActiveHarnessModels: async () => [activeModel("flash", "deepseek"), selected],
    getHarnessModelSelection: () => selection.promise,
  });
  host.render(); host.reads[0].resolve(settings); await tick();
  assert.equal(host.render().modelOptions.length, 2);
  assert.equal(host.render().selectedModel, "");
  selection.resolve(selected); await tick();
  assert.equal(host.render().selectedModel, modelId(selected));
  assert.deepEqual(host.writes, []);
  host.unmount();
});

test("an empty selection never substitutes a default DeepSeek model", async () => {
  const legacy = activeModel("flash", "deepseek");
  const host = settingsHost({
    listActiveHarnessModels: async () => [activeModel("other"), legacy],
    getHarnessModelSelection: async () => null,
  });
  host.render(); host.reads[0].resolve(settings); await tick();
  assert.equal(host.render().selectedModel, "");
  assert.deepEqual(host.writes, []);
  host.unmount();
});

test("a late saved-selection read cannot replace a newer manual model choice", async () => {
  const staleSelection = deferred(), first = activeModel("flash", "deepseek"), next = activeModel("pro", "deepseek");
  let selectionReads = 0;
  const host = settingsHost({
    getHarnessModelSelection: () => {
      selectionReads++;
      return selectionReads === 2 ? staleSelection.promise : Promise.resolve(selectionReads === 1 ? first : next);
    },
  });
  host.render(); host.reads[0].resolve(settings); await tick();
  host.changed({ reason: "chat_model" });
  host.render().chooseModel(modelId(next));
  staleSelection.resolve(first); host.reads[1].resolve(settings); await tick();
  assert.equal(host.render().selectedModel, modelId(next));
  assert.equal(host.writes.length, 1);
  host.writes[0].resolve(next); await tick();
  host.reads[2].resolve(settings); await tick();
  assert.equal(host.render().selectedModel, modelId(next));
  assert.equal(host.render().switching, false);
  host.unmount();
});

test("related refreshes retain the last saved selection while unrelated settings do not reload", async () => {
  const nextSelection = deferred();
  const selected = activeModel("chosen");
  let selectionReads = 0;
  const host = settingsHost({
    listActiveHarnessModels: async () => [selected],
    getHarnessModelSelection: () => ++selectionReads === 1 ? Promise.resolve(selected) : nextSelection.promise,
  });
  host.render(); host.reads[0].resolve(settings); await tick();
  for (const reason of ["sandbox", "terminal_shell", "web_search", "output_style"]) host.changed({ reason });
  assert.equal(host.reads.length, 1);
  assert.equal(selectionReads, 1);
  host.changed({ reason: "models" });
  host.reads[1].resolve(settings); await tick();
  assert.equal(host.render().selectedModel, modelId(selected));
  nextSelection.reject(new Error("selection refresh failed")); await tick();
  assert.equal(host.render().selectedModel, modelId(selected));
  assert.equal(host.render().modelOptions[0].label, selected.displayName);
  assert.equal(host.errors.length, 1);
  host.unmount();
});

test("startup keeps completed onboarding without a model but reopens when the key is missing", async () => {
  const source = fs.readFileSync(path.join(root, "src/App.tsx"), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let effect;
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useEffect"
      && node.arguments[0]?.getText(ast).includes("listHarnessProviders()")) effect = node.arguments[0].getText(ast);
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(effect);
  for (const [complete, providers, opens, reads] of [
    ["true", [{ enabled: true, hasApiKey: true }], false, 1],
    ["true", [{ enabled: true, hasApiKey: false }], true, 1],
    ["true", [{ enabled: false, hasApiKey: true }], true, 1],
    [undefined, [{ enabled: true, hasApiKey: true }], false, 0],
  ]) {
    const onboarding = [], local = new Map([["onboarding_complete", complete]]);
    let providerReads = 0;
    vm.runInNewContext(`(${effect})()`, {
      listHarnessProviders: async () => { providerReads++; return providers; },
      setShowOnboarding: value => onboarding.push(value),
      localStorage: { getItem: key => local.get(key), removeItem: key => local.delete(key) },
    });
    await tick();
    assert.equal(providerReads, reads);
    assert.deepEqual(onboarding, opens ? [true] : []);
    assert.equal(local.get("onboarding_complete"), opens ? undefined : complete);
  }
});

test("idle composer renders keep the model list stable for suggestion effects", async () => {
  const host = settingsHost();
  const initial = host.render();
  assert.strictEqual(host.render().models, initial.models, "idle renders must not retrigger model-dependent effects");
  host.reads[0].resolve(settings);
  await tick();
  const loaded = host.render();
  assert.deepEqual(Array.from(loaded.models), ["flash", "pro"]);
  assert.notStrictEqual(loaded.models, initial.models, "a real list change must update consumers");
  assert.strictEqual(host.render().models, loaded.models);
});

test("the thinking preset preserves initial and saved selections without writing off", async () => {
  for (const effort of ["none", "low", "medium", "high", "max"]) {
    const host = settingsHost({ getHarnessModelSelection: async () => ({ ...activeModel("flash", "deepseek"), reasoningEffort: effort }) });
    host.render();
    host.reads[0].resolve({ ...settings, reasoning_effort: effort });
    await tick();
    assert.equal(host.render().selectedThinking, effort === "none" ? "off" : effort);
    assert.deepEqual(host.writes, []);
    host.unmount();
  }
});

test("an empty active registry never revives inactive models from legacy settings", async () => {
  const host = settingsHost({ listActiveHarnessModels: async () => [], getHarnessModelSelection: async () => null });
  host.render();
  host.reads[0].resolve(settings);
  await tick();
  assert.equal(host.render().modelOptions.length, 0);
  assert.equal(host.render().selectedModel, "");
  assert.deepEqual(host.writes, []);
  host.unmount();
});

test("slash candidates exclude a retained selection absent from the active registry", async () => {
  const selected = {
    providerId: "retired", modelName: "retained-model", displayName: "Retained",
    providerChannel: "Retired provider", contextWindow: 1000,
    thinkingLevels: ["off", "high"], thinkingDistinct: [], thinkingDisable: true,
  };
  const host = settingsHost({ listActiveHarnessModels: async () => [], getHarnessModelSelection: async () => selected });
  host.render();
  host.reads[0].resolve(settings);
  await tick();
  assert.equal(host.render().modelOptions.length, 1, "capsule can retain its pending selection");
  assert.equal(host.render().activeModelOptions.length, 0, "slash uses only activated routes");
  host.unmount();
});

test("unselected active models are listed without claiming the runtime selected the first one", async () => {
  const host = settingsHost({ listActiveHarnessModels: async () => [{
    providerId: "other", modelName: "other-model", displayName: "Other", providerChannel: "Other provider",
    contextWindow: 1000, thinkingLevels: ["off", "high"], thinkingDistinct: [], thinkingDisable: true,
  }], getHarnessModelSelection: async () => null });
  host.render();
  host.reads[0].resolve(settings);
  await tick();
  assert.equal(host.render().modelOptions.length, 1);
  assert.equal(host.render().selectedModel, "");
  assert.deepEqual(host.writes, []);
  host.unmount();
});

function thinkingSlider(props = {}) {
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(path.join(root, "src/components/ModelThinkingSelector.tsx"), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  vm.runInNewContext(code, {
    module, exports: module.exports,
    require(name) {
      if (name === "react") return { useId: () => "thinking-selector" };
      if (name === "react-i18next") return { useTranslation: () => ({ t: (key, options) =>
        key === "composer.thinkingEquivalent" ? `${options.level}·等价` : key }) };
      if (name === "react/jsx-runtime") return { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }) };
      if (name === "lucide-react") return { Check: "Check", ChevronDown: "ChevronDown", CircleHelp: "CircleHelp" };
      if (name === "./shadcn/utils") return { cn: (...values) => values.filter(Boolean).join(" ") };
      const component = name.split("/").at(-1);
      return { [component]: component, MENU_ITEM_ATTR: "data-menu-item" };
    },
  });
  const element = module.exports.ModelThinkingSelector({
    open: true, models: [], selectedModel: "", selectedThinking: "high",
    thinkingOptions: ["off", "minimal", "low", "medium", "high", "xhigh", "max"].map(id => ({ id, label: id })),
    selectModelLabel: "model", noModelsLabel: "empty", onOpenChange() {}, onChooseModel() {}, onChooseThinking() {},
    ...props,
  });
  function find(node) {
    if (Array.isArray(node)) return node.map(find).find(Boolean);
    if (!node || typeof node !== "object") return undefined;
    return node.type === "EffortSlider" ? node.props : find(node.props?.children);
  }
  const slider = find(element);
  return slider;
}

test("thinking off follows the model's ability to disable, independently of effort levels", () => {
  for (const thinkingDisable of [true, false]) {
    const slider = thinkingSlider({ thinkingLevels: ["off", "low", "high"], thinkingDisable });
    assert.equal(slider.stops.some(stop => stop.value === "off"), thinkingDisable);
    assert.equal(slider.stops.some(stop => stop.value === "low"), true);
    assert.equal(slider.stops.some(stop => stop.value === "medium"), false);
    assert.equal(slider.stops[0].value, thinkingDisable ? "off" : "low");
  }
});

test("models without resolved reasoning capabilities never expose a fake off control", () => {
  assert.equal(thinkingSlider(), undefined);
});

test("explicitly empty thinking levels remain unavailable", () => {
  assert.equal(thinkingSlider({ thinkingLevels: [] }), undefined);
});

test("selector can read the selected model's close-thinking capability", () => {
  const slider = thinkingSlider({ selectedModel: "chosen", models: [{
    id: "chosen", label: "chosen", providerLabel: "provider", thinkingLevels: ["off", "high"], thinkingDisable: true,
  }] });
  assert.equal(slider.stops.some(stop => stop.value === "off"), true);
});

test("h029 equivalent levels are merged without exposing a provider default choice", () => {
  const chosen = [];
  const slider = thinkingSlider({ selectedModel: "chosen", models: [{
    id: "chosen", label: "chosen", providerLabel: "provider",
    thinkingLevels: ["off", "low", "medium", "high", "max"],
    thinkingDistinct: ["off", "low", "high", "max"], thinkingDisable: true,
  }], onChooseThinking: value => chosen.push(value) });
  assert.deepEqual(Array.from(slider.stops, stop => stop.value), ["off", "low", "high", "max"]);
  assert.equal(slider.stops.find(stop => stop.value === "high").label, "high");
  assert.equal(slider.stops.find(stop => stop.value === "off").label, "off");
  slider.onChange("medium");
  assert.deepEqual(chosen, ["medium"]);
});

test("an entirely disabled thinking slider rejects keyboard and pointer commits", () => {
  const module = { exports: {} };
  const element = (type, props) => ({ type, props });
  const hooks = {
    useState: initial => [typeof initial === "function" ? initial() : initial, () => {}],
    useRef: current => ({ current }), useEffect() {}, useLayoutEffect() {},
  };
  const code = ts.transpileModule(fs.readFileSync(path.join(root, "src/components/ui/EffortSlider.tsx"), "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports,
    window: { matchMedia: () => ({ matches: false }) },
    require(name) {
      if (name === "react") return hooks;
      if (name === "react/jsx-runtime") return { jsx: element, jsxs: element };
      if (name === "../shadcn/slider") return { Slider: "Slider" };
      if (name === "../shadcn/button") return { Button: "Button" };
      if (name === "./effortMotion") return { createEffortMotion() {} };
      throw new Error(`Unexpected import: ${name}`);
    },
  });
  const writes = [];
  const rendered = module.exports.EffortSlider({
    stops: ["off", "low", "high"].map(value => ({ value, label: value, disabled: true })),
    value: "off", onChange: value => writes.push(value), ariaLabel: "thinking",
  });
  const slider = rendered.props.children.find(child => child.type === "Slider").props;
  slider.onKeyDownCapture({ key: "ArrowRight", preventDefault() {}, stopPropagation() {} });
  slider.onValueCommit([2]);
  assert.deepEqual(writes, []);
  assert.equal(slider.disabled, true);
});

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
  let selection = { ...activeModel("flash", "deepseek"), reasoningEffort: "high" };
  const host = settingsHost({ getHarnessModelSelection: async () => selection });
  host.render(); host.reads[0].resolve(settings); await tick();
  host.changed();
  host.render().chooseThinking("max");
  selection = { ...activeModel("flash", "deepseek"), reasoningEffort: "max" };
  host.writes[0].resolve({ ...settings, reasoning_effort: "max" }); await tick();
  host.reads[1].resolve(settings); await tick();
  assert.equal(host.render().selectedThinking, "max");
  host.reads[2].resolve({ ...settings, reasoning_effort: "max" }); await tick();
  selection = { ...activeModel("pro", "deepseek"), reasoningEffort: "low" };
  host.changed(); host.reads[3].resolve({ ...settings, chat_model: "pro", reasoning_effort: "low" }); await tick();
  assert.equal(host.render().selectedModel, modelId(activeModel("pro", "deepseek")));
  assert.equal(host.render().selectedThinking, "low");
  host.unmount();
});

test("model and effort writes share a queue and navigation does not discard an accepted choice", async () => {
  const host = settingsHost();
  host.render(); host.reads[0].resolve(settings); await tick();
  host.render().chooseModel(modelId(activeModel("pro", "deepseek")));
  host.render().chooseThinking("max");
  assert.equal(host.writes.length, 1);
  assert.equal(host.render().selectedModel, modelId(activeModel("pro", "deepseek")));
  assert.equal(host.render().selectedThinking, "max");
  host.unmount();
  host.writes[0].resolve({ ...settings, chat_model: "pro" }); await tick();
  assert.equal(host.writes[1].value, "max");
  host.writes[1].resolve({ ...settings, chat_model: "pro", reasoning_effort: "max" }); await tick();
  assert.equal(host.reads.length, 1, "no reads after unmount");
});

test("medium is persisted without changing it into high", async () => {
  const host = settingsHost();
  host.render(); host.reads[0].resolve(settings); await tick();
  host.render().chooseThinking("medium");
  assert.equal(host.writes[0].value, "medium");
  host.writes[0].resolve({ ...settings, reasoning_effort: "medium" }); await tick();
  assert.equal(host.render().selectedThinking, "medium");
  host.unmount();
});

test("switching models drops the old effort and unsupported choices never write", async () => {
  const first = { ...activeModel("flash", "deepseek"), reasoningEffort: "max" };
  const next = { ...activeModel("pro", "deepseek"), thinkingLevels: ["low"], thinkingDisable: false };
  const host = settingsHost({ listActiveHarnessModels: async () => [first, next], getHarnessModelSelection: async () => first });
  host.render(); host.reads[0].resolve(settings); await tick();
  assert.equal(host.render().selectedThinking, "max");
  host.render().chooseModel(modelId(next));
  assert.equal(host.render().selectedThinking, null, "pending model switches must not invent an effort when no application level was received");
  host.render().chooseThinking("high");
  assert.equal(host.writes.length, 1);
  assert.equal(host.writes[0].reasoningEffort, null);
  host.unmount();
});

test("an explicitly disabled model never inherits a legacy global high or claims thinking is off", async () => {
  const selection = { ...activeModel("unknown"), thinkingLevels: [], reasoningEffort: null };
  const host = settingsHost({ getHarnessModelSelection: async () => selection });
  host.render(); await tick();
  assert.equal(host.render().selectedThinking, null);
  host.reads[0].resolve({ ...settings, reasoning_effort: "high" }); await tick();
  assert.equal(host.render().selectedThinking, null);
  host.render().chooseThinking("none");
  assert.equal(host.writes.length, 0);
  host.unmount();
});

test("h029 discarded provider default cannot be selected or persisted", async () => {
  const selected = { ...activeModel("flash", "deepseek"), reasoningEffort: "max" };
  const host = settingsHost({ getHarnessModelSelection: async () => selected });
  host.render(); host.reads[0].resolve(settings); await tick();
  host.render().chooseThinking("default");
  assert.deepEqual(host.writes, []);
  assert.equal(host.render().selectedThinking, "max");
  host.unmount();
});

test("switching to a declared high default selects high immediately and still accepts another level", async () => {
  const first = { ...activeModel("flash", "deepseek"), reasoningEffort: "max" };
  const next = { ...activeModel("qwen-4", "custom"), defaultReasoningEffort: "high", reasoningEffort: null };
  const host = settingsHost({ listActiveHarnessModels: async () => [first, next], getHarnessModelSelection: async () => first });
  host.render(); host.reads[0].resolve(settings); await tick();
  host.render().chooseModel(modelId(next));
  assert.equal(host.render().selectedThinking, "high", "the capsule and slider must select the target model default before the response");
  host.render().chooseThinking("low");
  assert.equal(host.render().selectedThinking, "low", "model default must not make the control read-only");
  host.unmount();
});
