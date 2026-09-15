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

const base = "../src/canvas/workflow/";
const schemas = loadTypeScript(`${base}utils/nodeSchemas.ts`);
const configSchema = loadTypeScript(`${base}utils/configSchema.ts`);
const registry = loadTypeScript(`${base}utils/nodeRegistry.ts`, { "./nodeSchemas": schemas, "./configSchema": configSchema });
const variables = loadTypeScript(`${base}utils/workflowVariables.ts`, { "./nodeRegistry": registry });
const snippets = loadTypeScript(`${base}utils/workflowSnippets.ts`, { "./nodeRegistry": registry, "./workflowVariables": variables });
const { captureWorkflowFragment: capture, instantiateWorkflowFragment: insert, parseSnippetLibrary: parse } = snippets;
const node = (id, kind = "llm", data = {}, extra = {}) => ({
  id, type: `professional-${kind}`, position: { x: 40, y: 80 }, selected: true,
  data: { ...registry.createDefaultNodeData(kind), ...data }, ...extra,
});
const edge = (source, target, extra = {}) => ({ id: `edge-${source}-${target}`, source, target, ...extra });
const sample = () => capture([node("a")], []);
const library = (fragment = sample()) => ({ version: 1, snippets: [{ id: "snippet-a", name: "本地片段", createdAt: 1, ...fragment }] });
const loadProfessional = () => loadTypeScript(`${base}store/professionalStore.ts`, { "../utils/nodeRegistry": registry }).useProfessionalStore;
const loadLibrary = () => loadTypeScript(`${base}store/snippetStore.ts`, { "../utils/workflowSnippets": snippets });

function storageHarness(t, initial = null) {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  const state = { raw: initial, writes: 0, failRead: false, failWrite: false };
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: () => { if (state.failRead) throw new Error("unavailable"); return state.raw; },
    setItem: (_key, raw) => { if (state.failWrite) throw new Error("unavailable"); state.raw = raw; state.writes++; },
  } });
  t.after(() => { if (previous) Object.defineProperty(globalThis, "localStorage", previous); else delete globalThis.localStorage; });
  return state;
}

test("capture keeps only selected professional nodes, internal ports and relative layout", () => {
  const nodes = [node("a"), node("b", "end", {}, { position: { x: 200, y: 180 } }), node("outside", "llm", {}, { selected: false }), node("creative", "llm", {}, { type: "creative-image-gen" })];
  const edges = [edge("a", "b", { sourceHandle: "branch-yes", targetHandle: "input", selected: true, animated: true, data: { result: "runtime" } }), edge("a", "outside")];
  const before = structuredClone({ nodes, edges });
  const fragment = capture(nodes, edges);
  assert.deepEqual(fragment.nodes.map((n) => n.id), ["a", "b"]);
  assert.deepEqual(fragment.nodes.map((n) => n.position), [{ x: 0, y: 0 }, { x: 160, y: 100 }]);
  assert.deepEqual(fragment.edges, [edge("a", "b", { sourceHandle: "branch-yes", targetHandle: "input" })]);
  assert.deepEqual({ nodes, edges }, before);
  fragment.nodes[0].data.llmPrompt = "independent edit";
  assert.deepEqual(nodes, before.nodes);
});

test("empty or disconnected selection is rejected with actionable feedback", () => {
  assert.throws(() => capture([], []), /选择/);
  assert.throws(() => capture([node("a", "llm", {}, { selected: false })], []), /选择/);
  assert.throws(() => capture([node("a"), node("b")], []), /不连通/);
});

test("references to uncaptured nodes are rejected even deep inside arbitrary parameters", () => {
  assert.throws(() => capture([node("a", "tool", { toolParams: { list: [{ nested: "{{#missing.output.value#}}" }] } })], []), /missing.*同时选择/);
  assert.throws(() => capture([node("a", "llm", { llmPrompt: "{{#outside.text#}}" }), node("outside", "llm", {}, { selected: false })], []), /outside/);
});

test("nested canonical references remap exact node segments without cascading or plain-text replacement", () => {
  const params = { list: [{ nested: ["{{#a.text#}}/{{#aa.text.deep#}}", "a aa {{a.text}} {{#a#}}", "{{#a.text#}}{{#a.text#}}"] }], "{{#a.text#}}": "literal object key" };
  const graph = [node("a", "tool", { toolParams: params }), node("aa")];
  const before = structuredClone(graph);
  const fragment = capture(graph, [edge("a", "aa")]);
  const result = insert(fragment, 107, -23, [], []);
  const [a, aa] = result.nodes;
  assert.equal(a.data.toolParams.list[0].nested[0], `{{#${a.id}.text#}}/{{#${aa.id}.text.deep#}}`);
  assert.equal(a.data.toolParams.list[0].nested[1], "a aa {{a.text}} {{#a#}}");
  assert.equal(a.data.toolParams.list[0].nested[2], `{{#${a.id}.text#}}{{#${a.id}.text#}}`);
  assert.equal(a.data.toolParams["{{#a.text#}}"], "literal object key");
  assert.deepEqual(a.position, { x: 107, y: -23 });
  assert.deepEqual(graph, before);
  assert.equal(fragment.nodes[0].data.toolParams.list[0].nested[0], params.list[0].nested[0]);
});

test("graph cycles and port handles survive capture and insertion", () => {
  const fragment = capture([node("a"), node("b")], [edge("a", "b", { sourceHandle: "if:yes", targetHandle: null }), edge("b", "a", { sourceHandle: "loop-end", targetHandle: "input" })]);
  const result = insert(fragment, 0, 0, [], []);
  assert.equal(result.edges[0].sourceHandle, "if:yes");
  assert.equal(result.edges[0].targetHandle, null);
  assert.equal(result.edges[1].targetHandle, "input");
  assert.equal(result.edges[1].target, result.nodes[0].id);
  assert.equal(result.edges[1].source, result.nodes[1].id);
});

test("node and edge IDs remain unique even when the UUID source repeats and existing IDs collide", () => {
  const original = crypto.randomUUID;
  crypto.randomUUID = () => "same";
  try {
    const fragment = capture([node("a"), node("b")], [edge("a", "b"), edge("b", "a")]);
    const result = insert(fragment, 0, 0, [node("pro-snippet-same")], [edge("x", "y", { id: "pro-snippet-edge-same" })]);
    const ids = [...result.nodes, ...result.edges].map((item) => item.id);
    assert.equal(new Set(ids).size, 4);
    assert.ok(!ids.includes("pro-snippet-same"));
    assert.ok(!ids.includes("pro-snippet-edge-same"));
    assert.ok(ids.every((id) => ![...fragment.nodes, ...fragment.edges].some((item) => item.id === id)));
  } finally { crypto.randomUUID = original; }
});

test("one real Zustand update inserts the complete fragment and one undo/redo restores it atomically", () => {
  const store = loadProfessional();
  const initial = node("existing");
  store.setState({ nodes: [initial], edges: [], past: [], future: [] });
  const fragment = capture([node("a"), node("b")], [edge("a", "b")]);
  const result = insert(fragment, 30, 70, store.getState().nodes, []);
  const events = [];
  const unsubscribe = store.subscribe((state) => events.push([state.nodes.length, state.edges.length, state.past.length]));
  store.getState().insertFragment(result);
  unsubscribe();
  assert.deepEqual(events, [[3, 1, 1]]);
  assert.equal(store.getState().nodes[0].selected, false);
  assert.ok(store.getState().nodes.slice(1).every((n) => n.data.status === "idle" && n.selected));
  const saved = structuredClone(store.getState().nodes);
  result.nodes[0].data.llmPrompt = "caller edit";
  assert.deepEqual(store.getState().nodes, saved);
  store.getState().undo();
  assert.deepEqual(store.getState().nodes, [initial]);
  assert.deepEqual(store.getState().edges, []);
  store.getState().redo();
  assert.deepEqual(store.getState().nodes, saved);
  assert.equal(store.getState().edges.length, 1);
  store.getState().undo();
  store.getState().insertFragment(insert(fragment, 0, 0, store.getState().nodes, []));
  assert.equal(store.getState().future.length, 0);
});

test("failed fragment insertion leaves history, selection and graph untouched", () => {
  const store = loadProfessional();
  store.setState({ nodes: [node("collision")], edges: [], past: [], future: [] });
  const state = store.getState();
  assert.throws(() => state.insertFragment({ nodes: [node("collision")], edges: [] }), /冲突/);
  assert.equal(store.getState(), state);
  assert.throws(() => state.insertFragment({ nodes: [node("a")], edges: [edge("a", "external")] }), /外部/);
  assert.equal(store.getState(), state);
  assert.throws(() => state.insertFragment({ nodes: [], edges: [] }), /没有/);
  assert.equal(store.getState(), state);
});

test("schema whitelist omits runtime data, password fields and case-insensitive credential headers", () => {
  // Noncredential sentinel values only; never put real credentials in fixtures.
  const marker = "[omission-test]";
  const source = [node("hook", "trigger-webhook", { webhookAuthSecret: marker, webhookHeaders: { Authorization: marker, "pRoXy-AuThOrIzAtIoN": marker, COOKIE: marker, "X-API-Key": marker, Accept: "application/json" }, result: { runtime: true }, output: "runtime", status: "error", errorMessage: "runtime", executionTime: 42, customUnknown: "drop" })];
  const before = structuredClone(source);
  const fragment = capture(source, []);
  const data = fragment.nodes[0].data;
  for (const key of ["webhookAuthSecret", "result", "output", "status", "errorMessage", "executionTime", "customUnknown"]) assert.equal(Object.hasOwn(data, key), false, key);
  assert.equal(Object.hasOwn(fragment.nodes[0], "selected"), false);
  assert.deepEqual(data.webhookHeaders, { Accept: "application/json" });
  assert.ok(!JSON.stringify(fragment).includes(marker));
  assert.deepEqual(source, before);
  const http = capture([node("http", "http-request", { httpHeaders: { authorization: marker, Cookie: marker, "X-API-KEY": marker, Accept: "text/plain" } })], []);
  assert.deepEqual(http.nodes[0].data.httpHeaders, { Accept: "text/plain" });
});

test("nested password schemas are stripped recursively, but arbitrary tool parameters are retained", () => {
  const properties = registry.getNodeDefinition("tool").configSchema.properties;
  const previous = properties.toolParams;
  properties.toolParams = { type: "object", properties: { rows: { type: "array", items: { type: "object", properties: { hidden: { type: "string", "x-widget": "password" }, visible: { type: "string" } } } } } };
  try {
    const fragment = capture([node("a", "tool", { toolParams: { rows: [{ hidden: "[omission-test]", visible: "keep" }] } })], []);
    assert.deepEqual(fragment.nodes[0].data.toolParams, { rows: [{ visible: "keep" }] });
  } finally { properties.toolParams = previous; }
  const params = { freeForm: { nested: ["user content"] } };
  assert.deepEqual(capture([node("a", "tool", { toolParams: params })], []).nodes[0].data.toolParams, params);
});

test("parent/group references require full selection and remap with parent-relative positions", () => {
  const parent = node("parent", "iteration", { childNodeIds: ["child"] }, { position: { x: 100, y: 200 } });
  const child = node("child", "iteration-start", {}, { parentId: "parent", position: { x: 12, y: 34 } });
  assert.throws(() => capture([{ ...parent, selected: false }, child], []), /分组|未选中/);
  assert.throws(() => capture([parent, { ...child, selected: false }], []), /完整选择/);
  const fragment = capture([child, parent], []);
  const result = insert(fragment, 500, 600, [], []);
  assert.deepEqual(result.nodes[0].position, { x: 500, y: 600 });
  assert.deepEqual(result.nodes[1].position, { x: 12, y: 34 });
  assert.equal(result.nodes[1].parentId, result.nodes[0].id);
  assert.deepEqual(result.nodes[0].data.childNodeIds, [result.nodes[1].id]);
  const groupOnly = capture([node("p", "loop", {}, { childIds: ["c"] }), node("c")], []);
  const inserted = insert(groupOnly, 0, 0, [], []);
  assert.deepEqual(inserted.nodes[0].childIds, [inserted.nodes[1].id]);
  assert.throws(() => capture([{ ...parent, parentId: "child" }, child], []), /循环/);
});

test("malformed, unknown, unsafe and externally referencing stored items are rejected", () => {
  const mutate = (fn) => { const value = library(); fn(value); return JSON.stringify(value); };
  for (const raw of [
    "", "{", "null", JSON.stringify({ version: 2, snippets: [] }),
    mutate((v) => { v.snippets[0].nodes = [null]; }),
    mutate((v) => { v.snippets[0].nodes[0].position.x = "0"; }),
    mutate((v) => { v.snippets[0].nodes[0].data.kind = "future-kind"; }),
    mutate((v) => { v.snippets[0].nodes[0].data.llmPrompt = "{{#outside.text#}}"; }),
    mutate((v) => { v.snippets[0].nodes[0].data.result = { runtime: true }; }),
    mutate((v) => { v.snippets[0].nodes[0].data.llmPrompt = []; }),
    mutate((v) => { v.snippets[0].nodes[0].data = JSON.parse('{"kind":"llm","label":"x","__proto__":{"polluted":true}}'); }),
    mutate((v) => { v.snippets[0].extra = true; }),
    mutate((v) => { v.snippets.push(structuredClone(v.snippets[0])); }),
  ]) assert.throws(() => parse(raw));
  assert.equal({}.polluted, undefined);
  assert.deepEqual(parse(null), { version: 1, snippets: [] });
  const valid = library();
  assert.deepEqual(parse(JSON.stringify(valid)), valid);
});

test("stored credential-bearing headers are rejected rather than silently sanitized on read", () => {
  const value = library(capture([node("a", "http-request")], []));
  value.snippets[0].nodes[0].data.httpHeaders.Authorization = "[omission-test]";
  assert.throws(() => parse(JSON.stringify(value)), /敏感/);
});

test("library persistence survives reload, rejects duplicate names and deletes only the requested item", (t) => {
  const storage = storageHarness(t);
  const { useSnippetStore } = loadLibrary();
  const store = useSnippetStore.getState();
  assert.equal(store.load(), true);
  store.saveSnippet("  My Snippet  ", [node("a")], []);
  assert.equal(storage.writes, 1);
  assert.equal(JSON.parse(storage.raw).version, 1);
  assert.throws(() => store.saveSnippet("my snippet", [node("b")], []), /同名/);
  assert.equal(storage.writes, 1);
  store.saveSnippet("Other", [node("b")], []);
  const restored = loadLibrary().useSnippetStore;
  assert.equal(restored.getState().load(), true);
  assert.equal(restored.getState().snippets.length, 2);
  restored.getState().deleteSnippet(restored.getState().snippets[0].id);
  assert.deepEqual(JSON.parse(storage.raw).snippets.map((item) => item.name), ["Other"]);
  assert.throws(() => store.saveSnippet("  ", [node("a")], []), /名称/);
});

test("write failures never report saved state or remove items from memory", (t) => {
  const storage = storageHarness(t, JSON.stringify(library()));
  const { useSnippetStore } = loadLibrary();
  const store = useSnippetStore.getState();
  store.load();
  const before = useSnippetStore.getState().snippets;
  storage.failWrite = true;
  assert.throws(() => store.saveSnippet("New", [node("a")], []), /写入失败/);
  assert.throws(() => store.deleteSnippet(before[0].id), /写入失败/);
  assert.equal(useSnippetStore.getState().snippets, before);
  assert.equal(storage.writes, 0);
});

test("broken library and denied reads preserve original storage and block all mutations", (t) => {
  const storage = storageHarness(t, '{"version":99,"snippets":[]}');
  const { useSnippetStore } = loadLibrary();
  const store = useSnippetStore.getState();
  assert.equal(store.load(), false);
  assert.ok(useSnippetStore.getState().error);
  assert.throws(() => store.saveSnippet("New", [node("a")], []));
  assert.throws(() => store.deleteSnippet("snippet-a"));
  assert.equal(storage.raw, '{"version":99,"snippets":[]}');
  assert.equal(storage.writes, 0);
  storage.failRead = true;
  assert.equal(store.load(), false);
  assert.match(useSnippetStore.getState().error, /权限/);
  assert.throws(() => store.saveSnippet("New", [node("a")], []), /读取/);
  assert.equal(storage.writes, 0);
});

test("every mutation re-reads storage so stale in-memory items cannot overwrite broken data", (t) => {
  const storage = storageHarness(t);
  const { useSnippetStore } = loadLibrary();
  useSnippetStore.getState().saveSnippet("Saved", [node("a")], []);
  storage.raw = "broken later";
  const writes = storage.writes;
  assert.throws(() => useSnippetStore.getState().deleteSnippet(useSnippetStore.getState().snippets[0].id));
  assert.equal(storage.raw, "broken later");
  assert.equal(storage.writes, writes);
});

test("bounded library sizes, excessive nesting and cyclic config are rejected", () => {
  assert.throws(() => parse(" ".repeat(snippets.SNIPPET_LIMITS.libraryChars + 1)), /大小限制/);
  assert.throws(() => capture([node("a", "llm", { llmPrompt: "x".repeat(snippets.SNIPPET_LIMITS.itemChars) })], []), /过大/);
  let nested = {}; for (let i = 0; i < 40; i++) nested = { nested };
  assert.throws(() => capture([node("a", "tool", { toolParams: nested })], []), /嵌套/);
  const cyclic = {}; cyclic.self = cyclic;
  assert.throws(() => capture([node("a", "tool", { toolParams: cyclic })], []), /嵌套/);
  assert.throws(() => insert(sample(), NaN, 0, [], []), /位置/);
});
