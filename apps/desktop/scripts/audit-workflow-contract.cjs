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
  loaded.require = (specifier) => Object.hasOwn(imports, specifier) ? imports[specifier]
    : specifier === "./conditionBranches" ? loadTypeScript("../src/canvas/workflow/utils/conditionBranches.ts") : requireModule(specifier);
  loaded._compile(compiled.outputText, filename);
  return loaded.exports;
}

async function emitFixtures(names = ["input", "condition", "output", "template"], providerId) {
  const schemas = loadTypeScript("../src/canvas/workflow/utils/nodeSchemas.ts");
  const configSchema = loadTypeScript("../src/canvas/workflow/utils/configSchema.ts");
  const registry = loadTypeScript("../src/canvas/workflow/utils/nodeRegistry.ts", {
    "./nodeSchemas": schemas, "./configSchema": configSchema,
  });
  const { conditionBranchContract } = loadTypeScript("../src/canvas/workflow/utils/conditionBranches.ts");
  const fixtures = [];
  for (const name of names) {
    let branchCatalog;
    const creative = name.startsWith("creative-");
    const node = (id, kind, patch = {}) => ({ id, type: `professional-${kind}`, position: { x: 0, y: 0 }, data: { ...registry.createDefaultNodeData(kind), ...patch } });
    let nodes = [node("start", "start"), node("end", "end")];
    let edges = [{ id: "e1", source: "start", target: "end" }];
    if (creative) {
      nodes = [
        { id: "camera", data: { kind: "camera", cameraModel: "synthetic" } },
        { id: "lens", data: { kind: "lens", lensModel: "synthetic" } },
      ];
      edges = [{ id: "__canvas_entry_0", source: "camera", target: "lens" }];
      if (name === "creative-entry-collision") nodes.push({ id: "__canvas_start", data: { kind: "aperture", aperture: "f2.8" } });
      if (name === "creative-explicit-entry") {
        nodes.unshift(node("authored-entry", "start"));
        edges.unshift({ id: "authored-edge", source: "authored-entry", target: "camera" });
      }
      if (name === "creative-exit-collision") {
        nodes.push({ id: "__canvas_end", data: { kind: "aperture", aperture: "f2.8" } });
        edges[0].id = "__canvas_exit_0";
      }
      if (["creative-explicit-exit", "creative-explicit-boundaries"].includes(name)) {
        nodes.push(node("authored-exit", "end"));
        edges.push({ id: "authored-exit-edge", source: "lens", target: "authored-exit" });
      }
      if (name === "creative-explicit-boundaries") {
        nodes.unshift(node("authored-entry", "start"));
        edges.unshift({ id: "authored-entry-edge", source: "authored-entry", target: "camera" });
      }
      if (name === "creative-empty") { nodes = []; edges = []; }
      if (name === "creative-multiple-roots") nodes.push({ id: "independent-camera", data: { kind: "camera", cameraModel: "synthetic" } });
    } else if (name === "professional-missing-entry") {
      nodes = [node("end", "end")]; edges = [];
    } else if (name === "professional-missing-exit") {
      nodes = [node("start", "start")]; edges = [];
    } else if (name === "professional-unreachable-end") {
      edges = [];
    } else if (name === "professional-orphan") {
      nodes.unshift(node("orphan", "variable-assigner", { assignments: [] }));
    } else if (name === "professional-rooted-reordered") {
      nodes.reverse();
    } else if (name.startsWith("branch-")) {
      const single = name.startsWith("branch-single-");
      const ids = single ? [name.includes("-legacy-") ? "true" : name.includes("-uuid-") ? "d5b4fd30-7b79-4d25-9ff3-aba124fd73ae" : "if"] : ["red", "blue", "yellow"];
      const chosen = single ? (name.endsWith("-true") ? 0 : 1) : Number(name.slice("branch-multi-".length));
      const conditions = ids.map((id, index) => ({ id, logic: "and", items: [{ variable: index >= chosen && chosen < ids.length ? "ON" : "OFF", operator: "is", value: "ON" }] }));
      const contract = conditionBranchContract(conditions);
      const ports = [...contract.cases, contract.fallback];
      branchCatalog = { ...contract, expectedBranch: ports[chosen].id, expectedPath: `path${chosen}` };
      nodes = [node("start", "start"), node("decide", "if-else", { conditions }),
        ...ports.map((port, index) => node(`path${index}`, "template-transform", { templateScript: port.id, templateInputVariables: [] })),
        node("end", "end", { outputVariables: [{ name: "report", type: "string", value: "BRANCH_SETTLED" }] })];
      edges = [{ id: "entry", source: "start", target: "decide" }, ...ports.flatMap((port, index) => [
        { id: `choice${index}`, source: "decide", sourceHandle: port.id, target: `path${index}` },
        { id: `exit${index}`, source: `path${index}`, target: "end" },
      ])];
      // Old v1 files round-trip through the actual import/export path without
      // rewriting stored IDs, source handles or condition fields.
      const io = loadTypeScript("../src/canvas/workflow/utils/workflowImportExport.ts");
      const imported = io.parseWorkflowImport(JSON.stringify(io.exportWorkflow("professional", nodes, edges)));
      assert.equal(imported.ok, true);
      assert.deepEqual(imported.data.edges, edges);
      assert.deepEqual(imported.data.nodes.map(n => n.data), nodes.map(n => n.data));
      nodes = imported.data.nodes; edges = imported.data.edges;
    } else if (name.startsWith("condition-truth-")) {
      const [, , logic, left, right] = name.split("-");
      nodes = [node("start", "start", { inputVariables: [{ name: "payload", type: "object", default: { a: left === "true" ? "ON" : "OFF", b: right === "true" ? "ON" : "OFF" } }] }),
        node("decide", "if-else", { conditions: [{ id: "true", logic, items: [
          { variable: "{{#start.payload.a#}}", operator: "is", value: "ON" },
          { variable: "{{#start.payload.b#}}", operator: "is", value: "ON" },
        ] }] }), node("yes", "template-transform", { templateScript: "YES", templateInputVariables: [] }),
        node("no", "template-transform", { templateScript: "NO", templateInputVariables: [] }),
        node("end", "end", { outputVariables: [{ name: "report", type: "string", value: "DESKTOP_SETTLED" }] })];
      edges = [{ id: "a", source: "start", target: "decide" },
        { id: "y", source: "decide", sourceHandle: "true", target: "yes" },
        { id: "n", source: "decide", sourceHandle: "false", target: "no" },
        { id: "ye", source: "yes", target: "end" }, { id: "ne", source: "no", target: "end" }];
    } else if (name.startsWith("end-contract-")) {
      const type = name.slice("end-contract-".length);
      const value = { string: "DESKTOP_TYPED", number: 0, object: { nested: [false, null] }, array: [0, false, {}], invalid: "DESKTOP_WRONG" }[type];
      nodes[0].data.inputVariables = [{ name: "value", type: type === "invalid" ? "string" : type, default: value }];
      nodes[1].data.outputVariables = [{ name: "report", type: type === "invalid" ? "number" : type, value: "{{#start.value#}}" }];
    } else if (name.startsWith("target-")) {
      nodes = [node("start", "start", { inputVariables: [{ name: "value", type: "string", default: "DESKTOP_FRESH" }] }),
        node("source", "template-transform", { templateScript: "{{#start.value#}}", templateInputVariables: [] }),
        node("answer", "answer", { answerTemplate: "{{#source.output#}}" }), node("end", "end"),
        node("unrelated", "variable-assigner", { assignments: [] })];
      edges = [{ id: "a", source: "start", target: "source" }, { id: "b", source: "source", target: "answer" },
        { id: "c", source: "answer", target: "end" }, { id: "d", source: "start", target: "unrelated" }];
      nodes.reverse();
    } else if (name.startsWith("settlement-")) {
      nodes = [node("start", "start"), node("left-end", "end", {
        outputVariables: [{ name: "report", type: "string", value: "LEFT" }],
      })];
      edges = [{ id: "a", source: "start", target: "left-end" }];
      if (name === "settlement-tail") {
        nodes.push(node("tail", "variable-assigner", { assignments: [] }));
        edges.push({ id: "b", source: "start", target: "tail" });
      } else {
        nodes.push(node("right-end", "end", {
          outputVariables: [{ name: "report", type: "string", value: "RIGHT" }],
        }));
        edges.push({ id: "b", source: "start", target: "right-end" });
        if (name.startsWith("settlement-choice")) {
          nodes.push(node("choose", "if-else", { conditions: [{ id: "true", logic: "and", items: [
            { variable: "yes", operator: "is", value: "yes" },
          ] }] }));
          const selected = name === "settlement-choice-left";
          edges = [{ id: "a", source: "start", target: "choose" },
            { id: "b", source: "choose", target: "left-end", sourceHandle: selected ? "true" : "false" },
            { id: "c", source: "choose", target: "right-end", sourceHandle: selected ? "false" : "true" }];
        }
      }
    } else if (name.startsWith("input-contract")) {
      nodes[0].data.inputVariables = [
        { name: "value", type: "object", required: true, default: { note: "{{#missing.value#}}", nested: [false, 0] } },
        { name: "optional", type: "string", required: false },
        { name: "nil", type: "array", required: false, default: null },
        { name: "flag", type: "boolean", default: false },
        { name: "count", type: "number", minimum: 0, maximum: 0, default: 0 },
        { name: "mode", type: "string", options: ["", "safe"], minLength: 0, maxLength: 4, default: "" },
      ];
      if (name === "input-contract-missing") delete nodes[0].data.inputVariables[0].default;
      nodes[1].data.outputVariables = [{ name: "report", type: "object", value: "{{#start.value#}}" }];
    } else if (name.startsWith("aggregator-")) {
      const kind = name.slice("aggregator-".length);
      const values = kind === "number" ? [7, 8] : kind === "object" ? [{ k: 1 }, { k: 2 }] : ["FIRST", "SECOND"];
      const inputType = kind === "array" ? "string" : kind;
      nodes[0].data.inputVariables = [
        { name: "first", type: inputType, default: values[0] },
        { name: "second", type: inputType, default: values[1] },
      ];
      nodes.splice(1, 0, node("aggregate", "variable-aggregator", {
        aggregatorOutputType: kind, aggregatorVariables: ["{{#start.first#}}", "{{#start.second#}}"],
      }));
      nodes[2].data.outputVariables = [{ name: "report", type: kind, value: "{{#aggregate.output#}}" }];
      edges = [{ id: "a", source: "start", target: "aggregate" }, { id: "b", source: "aggregate", target: "end" }];
    } else if (name === "answer") {
      nodes[0].data.inputVariables = [{ name: "customer", type: "string", default: "Eighteen" }];
      nodes.splice(1, 0, node("answer", "answer", {
        answerTemplate: "你好 {{customer}}，{{#start.customer#}}！",
        answerVariables: [{ name: "customer", value: "{{#start.customer#}}" }],
      }));
      nodes[2].data.outputVariables = [{ name: "report", type: "string", value: "{{#answer.answer#}}" }];
      edges = [{ id: "a", source: "start", target: "answer" }, { id: "b", source: "answer", target: "end" }];
    } else if (name === "iteration") {
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
      if (creative && n.data.kind !== "start") continue;
      const issues = configSchema.validateSchemaValue(schemas.NODE_CONFIG_SCHEMAS[n.data.kind], n.data);
      assert.deepEqual(issues, [], `invalid desktop fixture ${name}/${n.id}`);
    }
    let workflow;
    let finish;
    let listenersReady;
    const ready = new Promise((resolve) => { listenersReady = resolve; });
    const state = { nodes, edges, updateNodeData: () => {} };
    const executor = loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
      "../store/canvasStore": { useCanvasStore: { getState: () => ({ mode: creative ? "creative" : "professional" }) } },
      "../store/professionalStore": { useProfessionalStore: { getState: () => state } },
      "../store/creativeStore": { useCreativeStore: { getState: () => state } },
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
    const before = JSON.stringify({ nodes, edges });
    const running = executor.runWorkflow(name.startsWith("target-") ? name.slice("target-".length) : undefined);
    await ready;
    finish({ payload: { run_id: "audit-serialized-workflow" } });
    await running;
    assert.ok(workflow);
    assert.equal(JSON.stringify({ nodes, edges }), before, "projection must not mutate persisted canvas data");
    const serializedNodes = workflow.definition.nodes;
    if (name.startsWith("target-")) assert.equal(workflow.target_node_id, name.slice("target-".length));
    if (name.startsWith("input-contract")) {
      assert.deepEqual(workflow.inputs, {}, "the backend alone resolves defaults");
      const declarations = serializedNodes.find((node) => node.id === "start").config.inputVariables;
      assert.equal(Object.hasOwn(declarations[1], "default"), false);
      assert.equal(declarations[2].default, null);
      assert.equal(declarations[3].default, false);
      assert.equal(declarations[4].default, 0);
      assert.equal(declarations[5].default, "");
    } else if (name.startsWith("aggregator-")) {
      const config = serializedNodes.find((node) => node.id === "aggregate").config;
      assert.equal(config.aggregatorOutputType, name.slice("aggregator-".length));
      assert.deepEqual(config.aggregatorVariables, ["{{#start.first#}}", "{{#start.second#}}"]);
      assert.equal(Object.hasOwn(config, "variables"), false);
    } else if (name === "answer") {
      const config = serializedNodes.find((node) => node.id === "answer").config;
      assert.equal(config.answerTemplate, "你好 {{customer}}，{{#start.customer#}}！");
      assert.deepEqual(config.answerVariables, [{ name: "customer", value: "{{#start.customer#}}" }]);
      assert.equal(Object.hasOwn(config, "text"), false);
      assert.equal(Object.hasOwn(config, "variables"), false);
    } else if (name === "iteration") {
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
    } else if (name === "template") {
      assert.equal(serializedNodes.find((node) => node.id === "inspect").config.templateScript, "EXPECTED_TEMPLATE");
    }
    fixtures.push({ name, workflow, ...(branchCatalog ? { branchCatalog } : {}) });
  }
  process.stdout.write(JSON.stringify(fixtures));
}

// Replay actual Kernel events through the production consumer, not a shadow mapper.
async function consumeKernelEvents(filename) {
  const { name, events, terminal, expected, workflow, branchCatalog } = JSON.parse(readFileSync(filename, "utf8"));
  const branchPorts = name.startsWith("branch-");
  const outputContract = name.startsWith("end-contract-");
  const conditionTruth = name.startsWith("condition-truth-");
  const target = outputContract || conditionTruth || branchPorts ? undefined : name.slice("target-".length);
  const handlers = new Map();
  const state = { nodes: (outputContract ? ["start", "end"] : conditionTruth ? ["start", "decide", "yes", "no", "end"] : ["start", "source", "answer", "end", "unrelated"]).map((id) => ({
    id, data: { kind: id === "start" || id === "end" ? id : "answer", status: "idle" }, position: { x: 0, y: 0 },
  })), edges: [], updateNodeData(id, patch) {
    this.nodes = this.nodes.map((node) => node.id === id ? { ...node, data: { ...node.data, ...patch } } : node);
  } };
  if (branchPorts) state.nodes = workflow.definition.nodes.map(node => ({ id: node.id, data: { ...node.config, kind: node.kind, status: "idle" }, position: { x: 0, y: 0 } }));
  const executor = loadTypeScript("../src/canvas/workflow/utils/workflowExecutor.ts", {
    "../store/canvasStore": { useCanvasStore: { getState: () => ({ mode: "professional" }) } },
    "../store/professionalStore": { useProfessionalStore: { getState: () => state } },
    "../store/creativeStore": { useCreativeStore: { getState: () => state } },
    "@tauri-apps/api/core": { invoke: async (command) => {
      assert.equal(command, "start_workflow"); return { run_id: "target-replay", accepted: true };
    } },
    "@tauri-apps/api/event": { listen: async (name, callback) => { handlers.set(name, callback); return () => handlers.delete(name); } },
  });
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const running = executor.runWorkflow(target);
  // Bound asynchronous listener registration and ACK without host resources.
  for (let i = 0; i < 20; i += 1) await new Promise((resolve) => setImmediate(resolve));
  assert.ok(handlers.has("chat://event") && handlers.has("session://completed"));
  for (const event of events) handlers.get("chat://event")({ payload: {
    run_id: "target-replay", payload: { type: "workflow_node", event },
  } });
  if (branchPorts) {
    const data = id => state.nodes.find(node => node.id === id).data;
    assert.equal(data("decide").status, "completed");
    assert.deepEqual(data("decide").result, { success: true, outputs: { branch: branchCatalog.expectedBranch } });
    for (let i = 0; i <= branchCatalog.cases.length; i += 1) {
      assert.equal(data(`path${i}`).status, `path${i}` === branchCatalog.expectedPath ? "completed" : "idle");
    }
    assert.equal(data("end").status, "completed");
    assert.deepEqual(data("end").result.outputs, { report: "BRANCH_SETTLED" });
    handlers.get("session://completed")({ payload: { run_id: "target-replay", status: "succeeded" } });
  } else if (conditionTruth) {
    const data = (id) => state.nodes.find((node) => node.id === id).data;
    assert.equal(data("decide").status, "completed");
    assert.deepEqual(data("decide").result, { success: true, outputs: { branch: String(expected) } });
    assert.equal(data("yes").status, expected ? "completed" : "idle");
    assert.equal(data("no").status, expected ? "idle" : "completed");
    assert.equal(data("end").status, "completed");
    assert.deepEqual(data("end").result.outputs, { report: "DESKTOP_SETTLED" });
    handlers.get("session://completed")({ payload: { run_id: "target-replay", status: "succeeded" } });
  } else if (outputContract) {
    const end = state.nodes.find((node) => node.id === "end").data;
    assert.equal(state.nodes[0].data.status, "completed");
    const invalid = name.endsWith("invalid");
    assert.equal(end.status, invalid ? "error" : "completed");
    if (invalid) {
      assert.equal(end.result, undefined);
      assert.match(end.errorMessage, /InvalidWorkflowOutput: node end field report expected number actual string/);
      assert.equal(end.errorMessage.includes("DESKTOP_WRONG"), false);
    } else {
      assert.equal(end.result.success, true);
      assert.deepEqual(end.result.outputs, events.find((event) => event.node_id === "end" && event.status === "completed").outputs);
    }
    handlers.get("session://completed")({ payload: { run_id: "target-replay", status: invalid ? "failed" : "succeeded", error: terminal.message } });
  } else {
    const completed = target === "start" ? ["start"] : target === "answer" ? ["start", "source", "answer"] : ["start", "source", "answer", "end"];
    for (const node of state.nodes) {
      // Existing UI contract maps backend Skipped to idle, retaining its reason.
      assert.equal(node.data.status, completed.includes(node.id) ? "completed" : "idle", node.id);
      if (!completed.includes(node.id)) assert.equal(node.data.errorMessage, "outside target recompute scope");
    }
    if (target !== "start") assert.equal(state.nodes.find((node) => node.id === "answer").data.result, "DESKTOP_FRESH");
    handlers.get("session://completed")({ payload: { run_id: "target-replay", status: "succeeded" } });
  }
  await running;
  assert.equal(handlers.size, 0);
}

module.exports = { loadTypeScript };
if (require.main === module) {
  if (["--consume-target-events", "--consume-output-events", "--consume-condition-events", "--consume-branch-events"].includes(process.argv[2]) && process.argv[3]) {
    consumeKernelEvents(process.argv[3]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-branch-fixtures") {
    emitFixtures(["branch-single-if-true", "branch-single-if-false", "branch-single-legacy-true", "branch-single-uuid-true", ...[0, 1, 2, 3].map(index => `branch-multi-${index}`)]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-condition-truth-fixtures") {
    emitFixtures(["and", "or"].flatMap((logic) => [false, true].flatMap((a) => [false, true].map((b) => `condition-truth-${logic}-${a}-${b}`)))).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-output-contract-fixtures") {
    emitFixtures(["end-contract-string", "end-contract-number", "end-contract-object", "end-contract-array", "end-contract-invalid"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-target-fixtures") {
    emitFixtures(["target-start", "target-answer", "target-end"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-settlement-fixtures") {
    emitFixtures(["settlement-tail", "settlement-parallel", "settlement-choice-left", "settlement-choice-right"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-reachability-fixtures") {
    emitFixtures(["creative-multiple-roots", "professional-unreachable-end", "professional-orphan", "professional-rooted-reordered", "iteration"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-exit-fixtures") {
    emitFixtures(["creative-exit", "creative-exit-collision", "creative-explicit-exit", "creative-explicit-boundaries", "creative-empty", "professional-missing-exit"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-entry-fixtures") {
    emitFixtures(["creative-entry", "creative-entry-collision", "creative-explicit-entry", "professional-missing-entry"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-input-contract-fixture") {
    emitFixtures(["input-contract", "input-contract-missing"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-aggregator-fixtures") {
    emitFixtures(["aggregator-array", "aggregator-string", "aggregator-number", "aggregator-object"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-answer-fixture") {
    emitFixtures(["answer"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-iteration-fixture") {
    emitFixtures(["iteration"]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-embedding-fixture" && process.argv[3]) {
    emitFixtures(["embedding"], process.argv[3]).catch((error) => { console.error(error); process.exitCode = 1; });
  } else if (process.argv[2] === "--emit-fixtures") {
    emitFixtures().catch((error) => { console.error(error); process.exitCode = 1; });
  } else {
    throw new Error("opt-in audit only: pass --emit-fixtures or --emit-embedding-fixture <provider-id>");
  }
}
