const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const ts = require("typescript");
const React = require("react");
const { renderToStaticMarkup } = require("react-dom/server");

function load(relative, imports = {}) {
  const filename = path.resolve(__dirname, relative);
  const compiled = ts.transpileModule(readFileSync(filename, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
    fileName: filename,
  });
  const loaded = new Module(filename, module);
  loaded.filename = filename;
  loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  const original = loaded.require.bind(loaded);
  loaded.require = (name) => Object.hasOwn(imports, name) ? imports[name] : original(name);
  loaded._compile(compiled.outputText, filename);
  return loaded.exports;
}

const base = "../src/canvas/workflow/";
const schemas = load(`${base}utils/nodeSchemas.ts`);
const configSchema = load(`${base}utils/configSchema.ts`);
const registry = load(`${base}utils/nodeRegistry.ts`, { "./nodeSchemas": schemas, "./configSchema": configSchema });

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = () => new Promise((resolve) => setImmediate(resolve));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
function graph(mode, label = "saved") {
  return {
    nodes: [1, 2].map((i) => ({
      id: `${mode}-${i}`, type: `${mode}-${mode === "creative" ? "text-gen" : "llm"}`,
      position: { x: i * 24, y: 48 },
      data: { kind: mode === "creative" ? "text-gen" : "llm", label, status: "completed", prompt: "USER", output: "AI", result: "AI" },
    })),
    edges: [{ id: `${mode}-edge`, source: `${mode}-1`, target: `${mode}-2` }],
  };
}

function fixture(database = {}, override = async () => {}) {
  const legacy = new Map();
  global.window = { __TAURI_INTERNALS__: {} };
  global.localStorage = { getItem: (key) => legacy.get(key) ?? null, removeItem: (key) => legacy.delete(key) };
  const creative = load(`${base}store/creativeStore.ts`, { "../utils/canvasPromptProfile": { canvasPromptProfilePatch: async () => null } }).useCreativeStore;
  const professional = load(`${base}store/professionalStore.ts`, { "../utils/nodeRegistry": registry }).useProfessionalStore;
  const calls = [];
  const invoke = async (command, args) => {
    calls.push({ command, ...structuredClone(args) });
    await override(command, args);
    if (command === "canvas_workflow_state_read") return structuredClone(database[args.mode] ?? null);
    if (command === "canvas_workflow_state_write") { database[args.mode] = structuredClone(args.graph); return {}; }
    throw new Error(command);
  };
  const persistence = load(`${base}utils/workflowPersistence.ts`, {
    "@tauri-apps/api/core": { invoke },
    "../store/creativeStore": { useCreativeStore: creative },
    "../store/professionalStore": { useProfessionalStore: professional },
  });
  return { ...persistence, creative, professional, legacy, calls, database };
}
const writes = (f) => f.calls.filter((call) => call.command === "canvas_workflow_state_write");

test("repeated initialization shares hydration; both real stores restore and continue saving", async () => {
  const f = fixture({ creative: graph("creative"), professional: graph("professional") });
  const first = f.whenWorkflowPersistenceReady();
  assert.equal(first, f.whenWorkflowPersistenceReady());
  await first;
  assert.equal(f.calls.length, 2);
  for (const [mode, store] of [["creative", f.creative], ["professional", f.professional]]) {
    assert.equal(store.getState().nodes[0].data.output, "AI");
    assert.equal(store.getState().edges.length, 1);
    store.getState().updateNodeData(`${mode}-1`, { output: "updated" });
  }
  await f.flushWorkflowPersistence();
  assert.equal(writes(f).length, 2);
  assert.equal(f.database.creative.nodes[0].data.output, "updated");
  assert.equal(f.database.professional.nodes[0].data.output, "updated");
  await f.whenWorkflowPersistenceReady();
  assert.equal(f.calls.filter((c) => c.command.endsWith("_read")).length, 2);
});

test("new nodes and edges save immediately on flush, then survive a new window", async () => {
  const database = {};
  const f = fixture(database);
  await f.whenWorkflowPersistenceReady();
  f.creative.setState(graph("creative", "new"));
  f.professional.setState(graph("professional", "new"));
  await f.flushWorkflowPersistence();
  assert.equal(writes(f).length, 2);
  const reopened = fixture(database);
  await reopened.whenWorkflowPersistenceReady();
  for (const store of [reopened.creative, reopened.professional]) {
    assert.equal(store.getState().nodes.length, 2);
    assert.equal(store.getState().nodes[0].data.label, "new");
    assert.deepEqual(store.getState().nodes[1].position, { x: 48, y: 48 });
    assert.equal(store.getState().edges.length, 1);
  }
  await reopened.flushWorkflowPersistence();
  assert.equal(writes(reopened).length, 0);
});

test("automatic debounce persists edits and ignores undo metadata without graph changes", async () => {
  const f = fixture();
  await f.whenWorkflowPersistenceReady();
  f.creative.setState(graph("creative"));
  f.professional.setState(graph("professional"));
  await sleep(600);
  assert.equal(writes(f).length, 2);
  f.creative.getState().pushHistory();
  await f.flushWorkflowPersistence();
  assert.equal(writes(f).length, 2);
});

test("edits made during a slow write are serialized and latest state wins", async () => {
  const barrier = deferred();
  let count = 0;
  const f = fixture({}, async (command) => {
    if (command.endsWith("_write") && ++count === 1) await barrier.promise;
  });
  await f.whenWorkflowPersistenceReady();
  f.creative.setState(graph("creative", "first"));
  const first = f.flushWorkflowPersistence();
  await tick();
  f.creative.getState().updateNodeData("creative-1", { label: "latest" });
  const second = f.flushWorkflowPersistence();
  await tick();
  assert.equal(writes(f).length, 1, "must not dispatch a concurrent write for the same mode");
  barrier.resolve();
  await Promise.all([first, second]);
  assert.deepEqual(writes(f).map((call) => call.graph.nodes[0].data.label), ["first", "latest"]);
  assert.equal(f.database.creative.nodes[0].data.label, "latest");
});

test("failed reads do not overwrite stored nodes and initialization can retry", async () => {
  let fail = true;
  const database = { creative: graph("creative") };
  const f = fixture(database, async (command) => {
    if (fail && command.endsWith("_read")) throw new Error("database unavailable");
  });
  await assert.rejects(f.whenWorkflowPersistenceReady(), /unavailable/);
  await f.flushWorkflowPersistence();
  assert.equal(writes(f).length, 0);
  fail = false;
  await f.whenWorkflowPersistenceReady();
  assert.equal(f.creative.getState().nodes.length, 2);
  assert.equal(writes(f).length, 0);
});

test("failed writes retain dirty state and flush rejects until a retry succeeds", async () => {
  let fail = true;
  const f = fixture({}, async (command) => {
    if (fail && command.endsWith("_write")) throw new Error("disk full");
  });
  await f.whenWorkflowPersistenceReady();
  f.creative.setState(graph("creative"));
  await assert.rejects(f.flushWorkflowPersistence(), /disk full/);
  assert.equal(f.database.creative, undefined);
  fail = false;
  await f.flushWorkflowPersistence();
  assert.equal(f.database.creative.nodes.length, 2);
});

test("legacy graph migration keeps its source until database write succeeds", async () => {
  let fail = true;
  const f = fixture({}, async (command) => {
    if (fail && command.endsWith("_write")) throw new Error("migration failed");
  });
  f.legacy.set("workflow-canvas-creative", JSON.stringify(graph("creative")));
  await assert.rejects(f.whenWorkflowPersistenceReady(), /migration failed/);
  assert.ok(f.legacy.has("workflow-canvas-creative"));
  fail = false;
  await f.whenWorkflowPersistenceReady();
  assert.equal(f.legacy.has("workflow-canvas-creative"), false);
  assert.equal(f.database.creative.nodes.length, 2);
});

test("malformed database records cannot silently become empty editable graphs", async () => {
  const f = fixture({ creative: { broken: true } });
  await assert.rejects(f.whenWorkflowPersistenceReady(), /数据无效/);
  await f.flushWorkflowPersistence();
  assert.equal(writes(f).length, 0);
  assert.deepEqual(f.database.creative, { broken: true });
});

test("closing waits for both graph and preferences, and save failure keeps the window open", async () => {
  const effects = [];
  let handler, closed = 0, prevented = 0, fail = true;
  const messages = [];
  const barrier = deferred();
  const f = fixture({}, async (command) => {
    if (fail && command.endsWith("_write")) throw new Error("disk full");
  });
  const hook = load(`${base}hooks/useWorkflowPersistence.ts`, {
    react: { useEffect: (effect) => effects.push(effect), useState: (initial) => [initial, () => {}] },
    "@tauri-apps/api/core": { isTauri: () => true },
    "@tauri-apps/api/window": { getCurrentWindow: () => ({
      onCloseRequested: async (callback) => { handler = callback; return () => { handler = undefined; }; },
      close: async () => { closed++; },
    }) },
    "../utils/workflowPersistence": f,
    "../utils/canvasPreferences": { flushCanvasPreferences: () => barrier.promise },
    "../../../components/message": { message: { error: (text) => messages.push(text) } },
  });
  hook.useWorkflowPersistence();
  const cleanups = effects.map((effect) => effect());
  await f.whenWorkflowPersistenceReady();
  await tick();
  f.creative.setState(graph("creative"));
  const event = { preventDefault: () => prevented++ };
  await handler(event);
  assert.equal(closed, 0);
  assert.equal(messages.length, 1);
  fail = false;
  const closing = handler(event);
  await tick();
  assert.equal(closed, 0, "must also wait for preferences");
  barrier.resolve();
  await closing;
  assert.equal(closed, 1);
  assert.equal(prevented, 2);
  assert.equal(f.database.creative.nodes.length, 2);
  for (const cleanup of cleanups) cleanup?.();
  await tick();
  assert.equal(handler, undefined);
});

const output = load(`${base}components/NodeTextOutput.tsx`);
const outputUtils = load(`${base}utils/nodeOutput.ts`);
for (const [filename, exported, kind] of [
  ["creative/TextGenNode", "TextGenContent", "text-gen"],
  ["creative/ScriptGenContent", "ScriptGenContent", "script-gen"],
  ["professional/ProfessionalNodes", "LLMContent", "llm"],
]) {
  const component = load(`${base}nodes/${filename}.tsx`, {
    "../../components/NodeTextOutput": output,
    "../../utils/nodeOutput": outputUtils,
    "../../utils/nodeRegistry": registry,
  })[exported];
  test(`${kind} body shows AI output, never prompt or system prompt`, () => {
    const data = { kind, status: "completed", prompt: "USER_INPUT", llmPrompt: "USER_INPUT", llmSystemPrompt: "SYSTEM_INPUT", output: "AI_OUTPUT", result: { success: true, outputs: { text: "AI_OUTPUT" } } };
    const html = renderToStaticMarkup(React.createElement(component, { data }));
    assert.ok(html.includes("AI_OUTPUT"));
    assert.ok(!html.includes("USER_INPUT") && !html.includes("SYSTEM_INPUT"));
    assert.ok(!html.includes("nodrag"), "正文不能禁用 React Flow 节点拖动");
    const empty = renderToStaticMarkup(React.createElement(component, { data: { ...data, output: undefined, result: undefined } }));
    assert.ok(empty.includes("运行后显示输出"));
    assert.ok(!empty.includes("USER_INPUT"));
  });
}

test("result projection handles text, answer, objects, zero and false; markdown exports output only", () => {
  for (const [result, expected] of [["hello", "hello"], [{ outputs: { answer: "yes" } }, "yes"], [false, "false"], [0, "0"], [{ outputs: { values: [1, 2] } }, JSON.stringify({ values: [1, 2] }, null, 2)], [undefined, undefined]]) {
    assert.equal(outputUtils.nodeResultText(result), expected);
  }
  assert.equal(outputUtils.nodeOutputMarkdown("title", "AI_OUTPUT"), "# title\n\nAI_OUTPUT");
  assert.equal(outputUtils.nodeOutputMarkdown("title", undefined), null);
});
