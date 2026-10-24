const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const { test } = require("node:test");
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

function graphStore() {
  let state = { nodes: [], edges: [] };
  const listeners = new Set();
  const setState = (patch) => {
    const previous = state;
    state = { ...state, ...patch };
    for (const listener of listeners) listener(state, previous);
  };
  return {
    getState: () => ({
      ...state,
      setNodes: (nodes) => setState({ nodes }),
      setEdges: (edges) => setState({ edges }),
    }),
    setState,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

function readReactFlowPersistenceProps() {
  const filename = path.resolve(__dirname, "../src/canvas/CanvasApp.tsx");
  const source = readFileSync(filename, "utf8");
  const ast = ts.createSourceFile(filename, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let result;
  const visit = (node) => {
    if (ts.isJsxOpeningElement(node) && node.tagName.getText(ast) === "ReactFlow") {
      const attributes = node.attributes.properties;
      result = {
        fitView: attributes.some((attribute) =>
          ts.isJsxAttribute(attribute) && attribute.name.getText(ast) === "fitView"),
        defaultViewport: attributes.some((attribute) =>
          ts.isJsxAttribute(attribute) && attribute.name.getText(ast) === "defaultViewport"),
      };
    }
    ts.forEachChild(node, visit);
  };
  visit(ast);
  assert.ok(result, "production ReactFlow element was not found");
  return result;
}

test("canvas viewport survives workflow graph save and reload", {
  skip: process.env.RUN_HARNESS_AUDIT !== "1",
  timeout: 5000,
}, async () => {
  const previousWindow = globalThis.window;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const previousLocalStorage = globalThis.localStorage;
  globalThis.localStorage = {
    getItem: () => null,
    removeItem: () => {},
  };

  try {
    const canvasStore = loadTypeScript("../src/canvas/workflow/store/canvasStore.ts").useCanvasStore;
    const creative = graphStore();
    const professional = graphStore();
    const documents = new Map();
    const writeLog = [];
    const invoke = async (command, args) => {
      if (command === "canvas_workflow_state_read") return documents.get(args.mode) ?? null;
      if (command === "canvas_workflow_state_write") {
        documents.set(args.mode, args.graph);
        writeLog.push({ ...args, graph: structuredClone(args.graph) });
        return {};
      }
      throw new Error(`unexpected Tauri command: ${command}`);
    };
    const makePersistence = () => loadTypeScript("../src/canvas/workflow/utils/workflowPersistence.ts", {
      "@tauri-apps/api/core": { invoke },
      "../store/creativeStore": { useCreativeStore: creative },
      "../store/professionalStore": { useProfessionalStore: professional },
    });

    const firstBoot = makePersistence();
    await firstBoot.whenWorkflowPersistenceReady();
    const savedViewport = { x: -975, y: -430, zoom: 1.65 };
    canvasStore.getState().setViewport(savedViewport);
    creative.setState({
      nodes: [{ id: "audit-node", type: "creative-text-gen", position: { x: 600, y: 400 }, data: { kind: "text-gen" } }],
      edges: [],
    });
    await firstBoot.flushWorkflowPersistence();

    const graphWrite = writeLog.find((entry) => entry.mode === "creative");
    assert.ok(graphWrite, "the graph edit should reach the formal persistence command");

    // A fresh module instance models a new WebView process and uses the production Zustand defaults.
    const restartedCanvasStore = loadTypeScript("../src/canvas/workflow/store/canvasStore.ts").useCanvasStore;
    const secondBoot = makePersistence();
    await secondBoot.whenWorkflowPersistenceReady();
    await firstBoot.flushWorkflowPersistence();
    const reactFlowProps = readReactFlowPersistenceProps();
    const restoredViewport = restartedCanvasStore.getState().viewport;
    const restoredGraphNodeIds = creative.getState().nodes.map((node) => node.id);
    console.log(JSON.stringify({
      savedViewport,
      persistedGraphKeys: Object.keys(graphWrite.graph).sort(),
      persistedViewport: graphWrite.graph.viewport ?? null,
      restoredGraphNodeIds,
      restoredViewport,
      reactFlowProps,
    }));

    assert.deepEqual(restoredGraphNodeIds, ["audit-node"], "the persisted graph itself should restore");

    assert.deepEqual(
      graphWrite.graph.viewport,
      savedViewport,
      "the durable canvas graph must retain its viewport for exact restoration",
    );
  } finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
    if (previousLocalStorage === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = previousLocalStorage;
  }
});
