const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const test = require("node:test");
const ts = require("typescript");

const settle = () => new Promise(resolve => setImmediate(resolve));
function settings(top_p = 0.2) {
  return { creativity: null, scene: null, temperature: null, top_p,
    max_output_tokens: null, top_logprobs: null, reasoning_effort: "high",
    text: null, tool_choice: null, user: null, developer: {}, ineffective: {} };
}
function preview(value = 0.95) {
  return [{ model: "backend-selected-model", advertised_tools: true, sampling: {
    configured_top_p: 0.2, effective_top_p: value, wire_top_p: value,
    thinking_enabled: true, tools_enabled: true, reason: "flash_thinking_floor",
  } }];
}
function nodes(tree) {
  const result = [];
  const visit = value => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (!value || typeof value !== "object" || !value.props) return;
    result.push(value); visit(value.props.children);
  };
  visit(tree); return result;
}

async function mount(initialView = { responses: settings(), responses_sampling: preview() }) {
  const cells = [], effects = [], writes = [], errors = [];
  let cursor = 0, first = true, tree;
  const useState = initial => {
    const index = cursor++;
    if (!(index in cells)) cells[index] = typeof initial === "function" ? initial() : initial;
    return [cells[index], value => { cells[index] = typeof value === "function" ? value(cells[index]) : value; }];
  };
  const hooks = { useState, useRef: initial => useState(() => ({ current: initial }))[0],
    useMemo: factory => factory(), useEffect: effect => { if (first) effects.push(effect); } };
  const api = {
    getSettings: async () => initialView,
    getResponsesSettings: async () => settings(),
    setResponsesSettings: value => new Promise((resolve, reject) => writes.push({ value, resolve, reject })),
    setResponsesDeveloperJson: async () => ({ responses: settings(0.94), responses_sampling: preview(0.973) }),
  };
  const stubs = new Proxy({}, { get: (_, key) => String(key) });
  const cache = new Map();
  function load(file) {
    if (cache.has(file)) return cache.get(file);
    let source = fs.readFileSync(file, "utf8");
    // Expose the production panel only to this in-memory test, not its public API.
    source = source.replace("function ResponsesSettingsPanel()", "export function ResponsesSettingsPanel()");
    const compiled = ts.transpileModule(source, { fileName: file, compilerOptions: {
      module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022,
      jsx: ts.JsxEmit.ReactJSX, esModuleInterop: true,
    } });
    const loaded = new Module(file, module);
    loaded.filename = file; loaded.paths = Module._nodeModulePaths(path.dirname(file));
    const nativeRequire = loaded.require.bind(loaded);
    loaded.require = name => {
      if (name === "react") return hooks;
      if (name === "react-i18next") return { useTranslation: () => ({ t: key => key }) };
      if (name === "../../api") return api;
      if (name === "../message") return { message: { error: value => errors.push(value), success() {} } };
      if (name === "@tauri-apps/api/app") return { getVersion: async () => "fixture" };
      if (name === "./useWebSearchSettings") return {};
      if (name === "./ResponsesSamplingPreview") return load(path.join(path.dirname(file), "ResponsesSamplingPreview.tsx"));
      if (name.startsWith("../shadcn/") || name.startsWith("../ui/")) return stubs;
      return nativeRequire(name);
    };
    loaded._compile(compiled.outputText, file); cache.set(file, loaded.exports); return loaded.exports;
  }
  const directory = path.resolve(__dirname, "../src/components/settings");
  const { ResponsesSettingsPanel } = load(path.join(directory, "ConfigSettings.tsx"));
  const { ResponsesSamplingPreview } = load(path.join(directory, "ResponsesSamplingPreview.tsx"));
  const render = () => { cursor = 0; tree = ResponsesSettingsPanel(); first = false; };
  render(); effects.forEach(effect => effect()); await settle(); render();
  const all = () => nodes(tree);
  return {
    render, writes, errors, samplingComponent: ResponsesSamplingPreview,
    rows: () => all().find(node => node.type === ResponsesSamplingPreview).props.rows,
    openAdvanced() { all().find(node => node.props.title === "settings.config.responses.advanced").props.onClick(); render(); },
    edit(value) { all().find(node => node.props.id === "responses-top_p").props.onChange({ target: { value } }); render(); },
    developerSave() {
      all().find(node => node.props.title === "settings.config.responses.developer").props.onClick(); render();
      return all().find(node => node.props.children === "settings.config.responses.validateSave").props.onClick();
    },
  };
}

test("H166 unsaved settings hide old sampling, and IPC values are consumed without UI inference", async () => {
  const ui = await mount();
  assert.deepEqual(ui.rows(), preview());
  ui.openAdvanced(); ui.edit("0.96");
  assert.equal(ui.rows(), null);
  assert.equal(ui.writes[0].value.top_p, 0.96);
  ui.writes[0].resolve({ responses: settings(0.96), responses_sampling: preview(0.973) });
  await settle(); ui.render();
  assert.deepEqual(ui.rows(), preview(0.973), "no front-end model rule may override backend evidence");
});

test("H166 rejected save restores the matching saved preview, not the candidate", async () => {
  const ui = await mount();
  ui.openAdvanced(); ui.edit("0.9");
  assert.equal(ui.rows(), null);
  ui.writes[0].reject(new Error("synthetic save rejection"));
  await settle(); ui.render();
  assert.deepEqual(ui.rows(), preview());
  assert.equal(ui.errors.length, 1);
});

test("H166 developer save refreshes evidence and absent old-host evidence is not invented", async () => {
  const ui = await mount();
  await ui.developerSave(); ui.render();
  assert.deepEqual(ui.rows(), preview(0.973));
  const old = await mount({ responses: settings() });
  assert.deepEqual(old.rows(), []);
});

test("H166 sampling rows render provenance using shadcn Items and no title tooltip", async () => {
  const ui = await mount();
  const tree = ui.samplingComponent({ rows: preview() });
  const all = nodes(tree);
  assert.equal(all[0].type, "ItemGroup");
  assert.equal(all.filter(node => node.props["data-sampling-model"]).length, 1);
  assert.ok(all.every(node => !("title" in node.props)));
  const text = all.flatMap(node => [node.props.children]).flat(Infinity).filter(value => typeof value === "string").join(" ");
  assert.ok(text.includes("settings.config.responses.sampling.reasons.flash_thinking_floor"));
});
