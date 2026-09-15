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

const registry = loadTypeScript("../src/canvas/workflow/utils/nodeRegistry.ts");
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
