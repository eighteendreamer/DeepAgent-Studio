const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const ts = require("typescript");

function loadTypeScript(relativePath, imports = {}) {
  const filename = path.resolve(__dirname, relativePath);
  const compiled = ts.transpileModule(readFileSync(filename, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
    fileName: filename,
  });
  const loaded = new Module(filename, module);
  loaded.filename = filename;
  loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  const requireModule = loaded.require.bind(loaded);
  loaded.require = (specifier) => Object.hasOwn(imports, specifier) ? imports[specifier] : requireModule(specifier);
  loaded._compile(compiled.outputText, filename);
  return loaded.exports;
}

const schemas = loadTypeScript("../src/canvas/workflow/utils/nodeSchemas.ts");
const configSchema = loadTypeScript("../src/canvas/workflow/utils/configSchema.ts");
const registry = loadTypeScript("../src/canvas/workflow/utils/nodeRegistry.ts", { "./nodeSchemas": schemas, "./configSchema": configSchema });
const variables = loadTypeScript("../src/canvas/workflow/utils/workflowVariables.ts", { "./nodeRegistry": registry });
const { createSchemaValue, reconcileArrayKeys, validateSchemaValue } = configSchema;
const {
  createDefaultNodeData,
  getAllNodeDefinitions,
  normalizeProfessionalData,
  normalizeProfessionalNode,
} = registry;

for (const definition of getAllNodeDefinitions()) {
  test(`new and legacy ${definition.kind} nodes have complete independent defaults`, () => {
    const fresh = createDefaultNodeData(definition.kind);
    assert.equal(fresh.kind, definition.kind);
    assert.equal(fresh.status, "idle");
    const legacy = {
      id: "persisted-node",
      type: `professional-${definition.kind}`,
      position: { x: 24, y: 48 },
      data: { label: "User label", description: "User configuration", custom: { value: 3 } },
    };
    const restored = normalizeProfessionalNode(legacy);
    assert.equal(restored.data.kind, definition.kind);
    assert.equal(restored.data.status, "idle");
    assert.equal(restored.data.label, "User label");
    assert.deepEqual(restored.data.custom, { value: 3 });
    assert.equal(legacy.data.kind, undefined);
    assert.deepEqual(normalizeProfessionalNode(restored), restored);
    assert.deepEqual(restored.position, legacy.position);
    assert.notEqual(createDefaultNodeData(definition.kind), fresh);
    for (const [key, value] of Object.entries(fresh)) {
      if (key !== "label") assert.deepEqual(restored.data[key], value);
    }
  });
}

test("normalization preserves configured empty arrays, zero, false and execution results", () => {
  const data = {
    kind: "llm", label: "Configured", status: "completed", llmTemperature: 0,
    llmPrompt: "", inputVariables: [], enabled: false, result: { text: "real result" },
  };
  const restored = normalizeProfessionalData(data, "professional-llm");
  for (const [key, value] of Object.entries(data)) assert.deepEqual(restored[key], value);
});

test("explicitly undefined legacy kind/status are repaired", () => {
  const restored = normalizeProfessionalData({ label: "Old", kind: undefined, status: undefined }, "professional-end");
  assert.equal(restored.kind, "end");
  assert.equal(restored.status, "idle");
  assert.deepEqual(restored.outputVariables, []);
});

test("creative and unknown node data is not silently converted", () => {
  const creative = { kind: "image-gen", label: "Image", status: "idle" };
  const unknown = { label: "Future node" };
  assert.equal(normalizeProfessionalData(creative, "creative-image-gen"), creative);
  assert.equal(normalizeProfessionalData(unknown, "professional-future"), unknown);
  assert.equal(normalizeProfessionalData(unknown, "end"), unknown);
  assert.equal(normalizeProfessionalData(unknown), unknown);
});

test("store import, edit, duplicate, new-node and undo paths retain normalized kinds", () => {
  const { useProfessionalStore } = loadTypeScript("../src/canvas/workflow/store/professionalStore.ts", {
    "../utils/nodeRegistry": registry,
  });
  const { parseWorkflowImport } = loadTypeScript("../src/canvas/workflow/utils/workflowImportExport.ts");
  const imported = parseWorkflowImport(JSON.stringify({
    version: 1, mode: "professional", edges: [], nodes: [{
      id: "pro-node-1", type: "professional-end", position: { x: 0, y: 0 },
      data: { label: "Old end", outputVariables: [{ name: "saved", type: "string", value: "kept" }] },
    }],
  }));
  assert.equal(imported.ok, true);
  const store = useProfessionalStore.getState();
  store.setNodes(imported.data.nodes);
  const restored = useProfessionalStore.getState().nodes[0];
  assert.equal(restored.data.kind, "end");
  assert.equal(restored.data.status, "idle");
  store.updateNodeData(restored.id, { label: "Edited end" });
  assert.equal(useProfessionalStore.getState().nodes[0].data.kind, "end");
  const copyId = store.addNodeAt("end", 48, 48, { ...restored.data, kind: undefined });
  const copy = useProfessionalStore.getState().nodes.find((n) => n.id === copyId);
  assert.notEqual(copyId, restored.id);
  assert.equal(copy.data.kind, "end");
  assert.deepEqual(copy.data.outputVariables, restored.data.outputVariables);
  const newId = store.addNode("llm", 100, 100);
  assert.equal(useProfessionalStore.getState().nodes.find((n) => n.id === newId).data.status, "idle");
  store.undo();
  store.redo();
  assert.ok(useProfessionalStore.getState().nodes.every((n) => n.data.kind && n.data.status));
});

for (const definition of getAllNodeDefinitions()) {
  test(`${definition.kind} configuration schema covers every persisted default field`, () => {
    assert.equal(definition.configSchema.type, "object");
    for (const key of Object.keys(definition.defaultData())) {
      if (key !== "label") assert.ok(definition.configSchema.properties[key], `${definition.kind}.${key}`);
    }
    assert.ok(definition.configSchema.properties.description);
    assert.doesNotThrow(() => JSON.stringify(definition.configSchema));
  });
}

test("upstream variable choices include transitive declarations but exclude self and unrelated nodes", () => {
  const node = (id, kind, data = {}) => ({ id, type: `professional-${kind}`, data: { ...createDefaultNodeData(kind), ...data }, position: { x: 0, y: 0 } });
  const nodes = [
    node("start", "start", { inputVariables: [{ name: "query", type: "string" }, { name: "files", type: "array" }] }),
    node("code", "code", { codeOutputVariables: [{ name: "count", type: "number" }] }),
    node("end", "end"), node("unrelated", "llm"),
  ];
  const edges = [{ source: "start", target: "code" }, { source: "code", target: "end" }];
  const available = variables.getAvailableVariables("end", nodes, edges);
  assert.deepEqual(available.map((v) => v.reference), ["{{#start.query#}}", "{{#start.files#}}", "{{#code.count#}}"]);
  assert.deepEqual(variables.getAvailableVariables("end", nodes, edges, ["array"]).map((v) => v.name), ["files"]);
  assert.equal(variables.getAvailableVariables("start", nodes, edges).length, 0);
  assert.deepEqual([...variables.getUpstreamNodeIds("end", [...edges, { source: "end", target: "start" }])].sort(), ["code", "start"]);
  assert.deepEqual(variables.getVariableReferences("Hi {{#start.query#}} / {{#code.count#}}"), [["start", "query"], ["code", "count"]]);
});

test("dynamic outputs follow edited declarations and do not leak empty fields", () => {
  assert.deepEqual(registry.getNodeOutputs("start", { inputVariables: [{ name: "", type: "string" }, { name: "new_name", type: "boolean" }] }).map((v) => [v.name, v.type]), [["new_name", "boolean"]]);
  assert.ok(registry.getNodeOutputs("parameter-extractor", { extractorParams: [{ name: "email", type: "string" }] }).some((v) => v.name === "email"));
  assert.ok(registry.getNodeOutputs("agent-v2", { agentV2Outputs: [{ name: "items", type: "array" }] }).some((v) => v.name === "items"));
  assert.equal(registry.getNodeOutputs("variable-aggregator", { aggregatorOutputType: "array" })[0].type, "array");
});

test("schema validation reports nested required, duplicate-name, type and range failures", () => {
  const schema = { type: "object", required: ["name"], properties: {
    name: { type: "string", minLength: 1 }, count: { type: "integer", minimum: 1, maximum: 4 },
    variables: { type: "array", items: { type: "object", required: ["name"], properties: { name: { type: "string" } } } },
  } };
  const issues = validateSchemaValue(schema, { count: 5, variables: [{ name: "same" }, { name: "same" }, {}] });
  assert.deepEqual(issues.map((i) => i.path).sort(), ["count", "name", "variables.1.name", "variables.2.name"]);
  assert.equal(validateSchemaValue({ type: "integer" }, 1.2).length, 1);
  assert.equal(validateSchemaValue({ type: "number" }, NaN).length, 1);
  assert.equal(validateSchemaValue({ type: "boolean" }, "false").length, 1);
  assert.equal(validateSchemaValue({ type: "boolean" }, false).length, 0);
  assert.equal(validateSchemaValue({ type: "number", minimum: 0 }, 0).length, 0);
});

test("new schema array entries have independent defaults and stable unique IDs", () => {
  const schema = { type: "object", required: ["id", "items", "enabled"], properties: {
    id: { type: "string", format: "uuid" }, items: { type: "array", default: [] }, enabled: { type: "boolean", default: false },
  } };
  const a = createSchemaValue(schema);
  const b = createSchemaValue(schema);
  assert.notEqual(a.id, b.id);
  assert.equal(a.enabled, false);
  a.items.push("changed");
  assert.deepEqual(b.items, []);
});

test("array row identity stays with its value after deletion and reordering", () => {
  const a = { name: "a", default: undefined };
  const b = { name: "b", default: undefined };
  assert.deepEqual(reconcileArrayKeys([a, b], ["key-a", "key-b"], [b]), ["key-b"]);
  assert.deepEqual(reconcileArrayKeys([a, b], ["key-a", "key-b"], [b, a]), ["key-b", "key-a"]);
  const replaced = reconcileArrayKeys([a], ["key-a"], [{ ...a }]);
  assert.notEqual(replaced[0], "key-a");
  assert.deepEqual(reconcileArrayKeys(["", ""], ["one", "two"], ["", ""]), ["one", "two"]);
});

test("built-in output names cannot be shadowed by user declarations", () => {
  for (const [kind, field, name] of [["agent-v2", "agentV2Outputs", "text"], ["parameter-extractor", "extractorParams", "__usage"], ["human-input", "humanInputFields", "action"]]) {
    const data = { ...createDefaultNodeData(kind), [field]: [{ name, type: "array", label: "collision", required: true }] };
    assert.ok(registry.getNodeConfigIssues(kind, data).some((issue) => issue.path === `${field}.0.name` && issue.message.includes("内置输出")));
    assert.equal(registry.getNodeOutputs(kind, data).filter((output) => output.name === name).length, 1);
  }
});

test("non-text model identifiers remain editable and header dictionaries validate types", () => {
  assert.equal(schemas.NODE_CONFIG_SCHEMAS["knowledge-index"].properties.indexEmbeddingModel["x-widget"], undefined);
  assert.equal(schemas.NODE_CONFIG_SCHEMAS["knowledge-retrieval"].properties.rerankModel["x-widget"], undefined);
  assert.equal(validateSchemaValue(schemas.NODE_CONFIG_SCHEMAS["http-request"].properties.httpHeaders, { Accept: 7 }).length, 1);
  assert.equal(validateSchemaValue(schemas.NODE_CONFIG_SCHEMAS.llm.properties.llmModel, "provider-1::custom-chat").length, 0);
});

/**
 * 供应商设置 store 的密钥落库行为：明文只进不出，且不需要任何“保存”按钮。
 * invoke 与 zustand 都被替换成本地假实现，写入参数可以直接观察。
 */
function createLocalStore(init) {
  let state;
  const set = (partial) => {
    state = { ...state, ...(typeof partial === "function" ? partial(state) : partial) };
  };
  const get = () => state;
  state = init(set, get);
  const api = () => state;
  api.getState = get;
  api.setState = set;
  return api;
}

function loadSettingsStore(calls) {
  const exports = loadTypeScript("../src/canvas/workflow/store/canvasSettingsStore.ts", {
    react: { useMemo: () => undefined },
    zustand: { create: createLocalStore },
    "@tauri-apps/api/core": {
      invoke: async (command, args) => {
        calls.push({ command, args });
        if (command !== "canvas_provider_save") return null;
        const provider = args.provider;
        return {
          id: provider.id || "p1",
          name: provider.name,
          protocol: provider.protocol,
          baseUrl: provider.baseUrl,
          apiKeySet: Boolean(provider.apiKey),
          models: provider.models ?? [],
        };
      },
    },
    "../utils/canvasPreferences": {
      onCanvasPreferenceChanged: () => () => {},
      readCanvasPreference: () => null,
      writeCanvasPreference: () => {},
    },
  });
  return exports.useCanvasSettingsStore;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function storeWithProvider() {
  const calls = [];
  const useStore = loadSettingsStore(calls);
  const store = useStore();
  const id = await store.addProvider({
    name: "Tolove",
    protocol: "openai",
    baseUrl: "https://toloveu.asia/v1",
    models: [],
  });
  calls.length = 0;
  return { calls, store, useStore, id };
}

function keyWrites(calls) {
  return calls
    .filter((call) => call.command === "canvas_provider_save")
    .map((call) => call.args.provider.apiKey);
}

test("api key typed into the provider form persists on its own, no save button", async () => {
  const { calls, store, useStore, id } = await storeWithProvider();
  await store.updateProvider(id, { apiKey: "  sk-auto-123  " });
  await sleep(700);
  assert.deepEqual(keyWrites(calls), ["sk-auto-123"]);
  assert.equal(useStore().providers.find((p) => p.id === id).apiKey, undefined);
});

test("flushProvider writes a pending key immediately and never rewrites it", async () => {
  const { calls, store, id } = await storeWithProvider();
  void store.updateProvider(id, { apiKey: "sk-pending" });
  await store.flushProvider(id);
  assert.deepEqual(keyWrites(calls), ["sk-pending"]);
  calls.length = 0;
  await sleep(600);
  assert.deepEqual(calls, []);
});

test("emptying the key field clears the stored secret instead of leaving it", async () => {
  const { calls, store, id } = await storeWithProvider();
  await store.updateProvider(id, { apiKey: "sk-ghost" });
  calls.length = 0;
  void store.updateProvider(id, { apiKey: "   " });
  await sleep(700);
  assert.ok(calls.some((call) => call.command === "canvas_secret_clear"));
  assert.ok(keyWrites(calls).length > 0);
  assert.ok(keyWrites(calls).every((key) => key === null));
});

const imageSize = loadTypeScript("../src/canvas/workflow/utils/imageSize.ts");
const canvasTypes = loadTypeScript("../src/canvas/workflow/types.ts");

test("image size is derived from ratio and resolution, the only value the kernel reads", () => {
  assert.equal(imageSize.imageSizeFor("1:1", "1K"), "1024x1024");
  assert.equal(imageSize.imageSizeFor("16:9", "2K"), "2048x1152");
  assert.equal(imageSize.imageSizeFor("9:16", "1K"), "576x1024");
  assert.equal(imageSize.imageSizeFor("Auto", "4K"), "auto");
  assert.equal(imageSize.imageSizeFor("custom", "1K", "1536x1024"), "1536x1024");
  // 自定义却没填值时交给供应商决定，而不是前端猜一个像素数。
  assert.equal(imageSize.imageSizeFor("custom", "1K", undefined), "auto");
  assert.equal(imageSize.imageSizeFor("3:4", "9K"), "768x1024");
  assert.deepEqual(imageSize.parseCustomSize("1024 × 512"), { w: 1024, h: 512 });
  assert.equal(imageSize.parseCustomSize("wide"), null);
});

test("the text category's 文生图 entry produces an image node, not a text carrier", () => {
  const textCategory = canvasTypes.CREATIVE_NODE_PICKER_CATEGORIES.find((item) => item.key === "text");
  const option = textCategory.options.find((item) => item.key === "text-to-image");
  assert.equal(option.kind, "image-gen");
  assert.equal(option.label, "文生图");
});

/**
 * 取消一次画布运行：必须通知内核，而不是只把前端监听摘掉。
 * 内核只认 start_workflow 回执里的 run id，所以这里同时钉住两个 id 是同一个。
 */
function loadWorkflowExecutor(calls, updates) {
  const creative = {
    getState: () => ({
      nodes: [{ id: "n1", data: { kind: "image-gen", status: "running" } }],
      edges: [],
      updateNodeData: (id, patch) => updates.push([id, patch]),
    }),
  };
  const professional = { getState: () => ({ nodes: [], edges: [], updateNodeData: () => {} }) };
  const canvas = { getState: () => ({ mode: "creative" }) };
  return loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
    "../store/canvasStore": { useCanvasStore: canvas },
    "../store/creativeStore": { useCreativeStore: creative },
    "../store/professionalStore": { useProfessionalStore: professional },
    "@tauri-apps/api/core": {
      invoke: async (command, args) => {
        calls.push({ command, args });
        return { run_id: "run_42", session_id: null, accepted: true };
      },
      convertFileSrc: (path) => path,
    },
    "@tauri-apps/api/event": { listen: async () => () => {} },
  });
}

test("cancelling a node run asks the kernel to cancel the same run id it returned", async () => {
  const previousWindow = globalThis.window;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const calls = [];
  const updates = [];
  try {
    const executor = loadWorkflowExecutor(calls, updates);
    const running = executor.runWorkflow("n1");
    await sleep(30);
    // 开跑时的 resetAllStatus 也会写一次 idle，取消前的复位要单独看。
    updates.length = 0;
    await executor.cancelWorkflow("n1");
    await running;
  } finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
  const started = calls.find((call) => call.command === "start_workflow");
  assert.ok(started, "画布运行必须由 start_workflow 发起");
  assert.deepEqual(
    calls.find((call) => call.command === "cancel_run")?.args,
    { runId: "run_42" },
  );
  assert.deepEqual(
    updates.filter(([, patch]) => patch.status === "idle").map(([id]) => id),
    ["n1"],
  );
});
