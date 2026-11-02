// Opt-in fixture bridge for workflow_audit.rs. Runs production TS serializers;
// only stores and the Tauri transport are replaced. No server/model is called.
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const ts = require("typescript");

// Same module-loading technique as check-workflow.test.cjs.
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

async function emitFixtures(names = ["input", "condition", "output", "template"], providerId) {
  const schemas = loadTypeScript("../src/canvas/workflow/utils/nodeSchemas.ts");
  const configSchema = loadTypeScript("../src/canvas/workflow/utils/configSchema.ts");
  const registry = loadTypeScript("../src/canvas/workflow/utils/nodeRegistry.ts", {
    "./nodeSchemas": schemas, "./configSchema": configSchema,
  });
  const fixtures = [];
  for (const name of names) {
    const node = (id, kind, patch = {}) => ({ id, type: `professional-${kind}`, position: { x: 0, y: 0 }, data: { ...registry.createDefaultNodeData(kind), ...patch } });
    let nodes = [node("start", "start"), node("end", "end")];
    let edges = [{ id: "e1", source: "start", target: "end" }];
    if (name === "iteration") {
      nodes[0].data.inputVariables = [{ name: "items", type: "array", default: ["ONE", "TWO"] }];
      nodes.splice(1, 0,
        node("iterate", "iteration", { inputVariable: "{{#start.items#}}", outputVariable: "{{#body.output#}}", parallel: true, maxConcurrency: 2 }),
        node("entry", "iteration-start"),
        node("body", "template-transform", { templateScript: "seen:{{#entry.item#}}", templateInputVariables: [] }),
      );
      edges = [
        { id: "a", source: "start", target: "iterate" },
        { id: "b", source: "iterate", sourceHandle: "item", target: "entry" },
        { id: "c", source: "entry", target: "body" },
        { id: "d", source: "iterate", sourceHandle: "done", target: "end" },
      ];
    } else if (name === "embedding") {
      nodes.splice(1, 0, node("embedding", "embeddings", {
        embeddingModel: `${providerId}::audit-embedding-model`, texts: ["audit embedding"],
      }));
      edges = [
        { id: "e1", source: "start", target: "embedding" },
        { id: "e2", source: "embedding", target: "end" },
      ];
    } else if (name === "input") {
      nodes[0].data.inputVariables = [{ name: "customer", type: "string", default: "BLUE" }];
      nodes[1].data.outputVariables = [{ name: "report", type: "string", value: "{{#start.customer#}}" }];
    } else if (name === "output") {
      nodes[1].data.outputVariables = [{ name: "report", type: "string", value: "EXPECTED_REPORT" }];
    } else {
      const kind = name === "condition" ? "if-else" : "template-transform";
      const patch = name === "condition"
        ? { conditions: [{ id: "true", logic: "and", items: [{ variable: "BLUE", operator: "is", value: "BLUE" }] }] }
        : { templateScript: "EXPECTED_TEMPLATE", templateInputVariables: [] };
      nodes.splice(1, 0, node("inspect", kind, patch));
      edges = [
        { id: "e1", source: "start", target: "inspect" },
        { id: "e2", source: "inspect", target: "end", ...(name === "condition" ? { sourceHandle: "true" } : {}) },
      ];
    }
    for (const n of nodes) {
      const issues = configSchema.validateSchemaValue(schemas.NODE_CONFIG_SCHEMAS[n.data.kind], n.data);
      assert.deepEqual(issues, [], `invalid desktop fixture ${name}/${n.id}`);
    }
    let workflow;
    let finish;
    let listenersReady;
    const ready = new Promise((resolve) => { listenersReady = resolve; });
    const state = { nodes, edges, updateNodeData: () => {} };
    const executor = loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
      "../store/canvasStore": { useCanvasStore: { getState: () => ({ mode: "professional" }) } },
      "../store/professionalStore": { useProfessionalStore: { getState: () => state } },
      "../store/creativeStore": { useCreativeStore: { getState: () => ({ nodes: [], edges: [] }) } },
      "@tauri-apps/api/core": { invoke: async (command, args) => {
        assert.equal(command, "start_workflow");
        workflow = args.workflow;
        return { run_id: "audit-serialized-workflow", session_id: null, accepted: true };
      } },
      "@tauri-apps/api/event": { listen: async (event, callback) => {
        if (event === "session://completed") { finish = callback; listenersReady(); }
        return () => {};
      } },
    });
    globalThis.window = { __TAURI_INTERNALS__: {} };
    const running = executor.runWorkflow();
    await ready;
    finish({ payload: { run_id: "audit-serialized-workflow" } });
    await running;
    assert.ok(workflow);
    const serializedNodes = workflow.definition.nodes;
    if (name === "iteration") {
      const config = serializedNodes.find((node) => node.id === "iterate").config;
      assert.equal(config.inputVariable, "{{#start.items#}}");
      assert.equal(config.outputVariable, "{{#body.output#}}");
      assert.equal(config.maxConcurrency, 2);
    } else if (name === "embedding") {
      const config = serializedNodes.find((node) => node.id === "embedding").config;
      assert.equal(config.embeddingModel, `${providerId}::audit-embedding-model`);
      assert.deepEqual(config.texts, ["audit embedding"]);
    } else if (name === "input") {
      assert.equal(serializedNodes.find((node) => node.id === "start").config.inputVariables[0].name, "customer");
    } else if (name === "condition") {
      assert.equal(serializedNodes.find((node) => node.id === "inspect").config.conditions[0].items[0].value, "BLUE");
    } else if (name === "output") {
      assert.equal(serializedNodes.find((node) => node.id === "end").config.outputVariables[0].name, "report");
    } else {
      assert.equal(serializedNodes.find((node) => node.id === "inspect").config.templateScript, "EXPECTED_TEMPLATE");
    }
    fixtures.push({ name, workflow });
  }
  process.stdout.write(JSON.stringify(fixtures));
}

module.exports = { loadTypeScript };
if (require.main === module) {
  if (process.argv[2] === "--emit-iteration-fixture") {
    emitFixtures(["iteration"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-embedding-fixture" && process.argv[3]) {
    emitFixtures(["embedding"], process.argv[3]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-fixtures") {
    emitFixtures().catch((error) => { console.error(error); process.exitCode = 1; });
  } else {
    throw new Error("opt-in audit only: pass --emit-fixtures or --emit-embedding-fixture <provider-id>");
  }
}
