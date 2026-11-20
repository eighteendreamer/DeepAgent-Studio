const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");

const source = fs.readFileSync(path.join(__dirname, "../src/components/OnboardingWizard.tsx"), "utf8");
const ast = ts.createSourceFile("OnboardingWizard.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let handler;
function visit(node) {
  if (ts.isVariableDeclaration(node) && node.name.getText(ast) === "handleConnect") handler = node.initializer.getText(ast);
  ts.forEachChild(node, visit);
}
visit(ast);
assert.ok(handler);
const compiled = ts.transpileModule(`module.exports = ${handler};`, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText;

function fixture(overrides = {}) {
  const calls = [], module = { exports: {} };
  const context = {
    module, apiKey: "synthetic-key", models: [], modelName: "",
    connectedKey: { current: "" }, connectPendingRef: { current: false }, movingRef: { current: false },
    mountedRef: { current: true }, reducedMotion: true, ONBOARDING_EASE: [],
    isTauri: () => true, console: { error() {} },
    setIsConnecting: value => calls.push(["busy", value]),
    setConnectError: value => calls.push(["error", value]),
    setConnectState() {}, connectAnimation: { start: async () => {} },
    setModels: models => calls.push(["models", models]),
    setModelName: value => calls.push(["modelName", value]),
    inputRef: { current: { focus() {} } }, goToStep: value => calls.push(["step", value]),
    listHarnessProviders: async () => [],
    saveHarnessProvider: async provider => calls.push(["provider", provider]),
    fetchHarnessModels: async id => { calls.push(["fetch", id]); return [{ modelName: "listed-model" }]; },
    setHarnessModelActive: async (...args) => calls.push(["active", ...args]),
    selectHarnessModel: async (...args) => calls.push(["select", ...args]),
    ...overrides,
  };
  vm.runInNewContext(compiled, context);
  return { connect: module.exports, calls, context };
}

test("onboarding lists registry models and requires an explicit choice before activation", async () => {
  const { connect, calls, context } = fixture();
  await connect();
  assert.deepEqual(calls.filter(call => ["fetch", "modelName"].includes(call[0])), [["fetch", "deepseek"], ["modelName", ""]]);
  assert.equal(context.connectedKey.current, "synthetic-key");
  assert.equal(calls.some(call => ["active", "select", "step"].includes(call[0])), false);
  const provider = calls.find(call => call[0] === "provider")[1];
  assert.equal(provider.id, "deepseek");
  assert.equal(provider.apiKey, "synthetic-key");
});

test("onboarding uses the exact selected registry identity without another discovery", async () => {
  const { connect, calls } = fixture({
    models: [{ modelName: "chosen-model" }], modelName: "chosen-model", connectedKey: { current: "synthetic-key" },
  });
  await connect();
  assert.deepEqual(calls.filter(call => ["active", "select", "step"].includes(call[0])), [
    ["active", "deepseek", "chosen-model", true], ["select", "deepseek", "chosen-model", true], ["step", 1],
  ]);
  assert.equal(calls.some(call => call[0] === "fetch" || call[0] === "provider"), false);
});

test("an in-flight registry listing admits one connection and failures cannot advance the wizard", async () => {
  let release, fetches = 0;
  const pending = new Promise((_, reject) => { release = () => reject(new Error("synthetic listing failure")); });
  const { connect, calls } = fixture({ fetchHarnessModels: () => { fetches++; return pending; } });
  const first = connect();
  await connect();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(fetches, 1);
  release();
  await first;
  assert.equal(calls.some(call => ["active", "select", "step"].includes(call[0])), false);
  assert.ok(calls.some(call => call[0] === "error" && call[1]));
});
