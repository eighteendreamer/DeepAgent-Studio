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
    module, apiKey: " synthetic-key ", providerId: "layso",
    connectPendingRef: { current: false }, movingRef: { current: false },
    mountedRef: { current: true }, reducedMotion: true, ONBOARDING_EASE: [],
    isTauri: () => true, console: { error() {} },
    setIsConnecting: value => calls.push(["busy", value]),
    setConnectError: value => calls.push(["error", value]),
    setConnectState() {}, connectAnimation: { start: async () => {} },
    inputRef: { current: { focus() {} } }, goToStep: value => calls.push(["step", value]),
    connectHarnessProvider: async (...args) => { calls.push(["connect", ...args]); },
    ...overrides,
  };
  vm.runInNewContext(compiled, context);
  return { connect: module.exports, calls };
}

for (const providerId of ["layso", "deepseek"]) {
  test(`a valid ${providerId} key advances directly to theme setup`, async () => {
    const { connect, calls } = fixture({ providerId });
    await connect();
    assert.deepEqual(calls.filter(call => ["connect", "step"].includes(call[0])), [
      ["connect", providerId, "synthetic-key"], ["step", 1],
    ]);
  });
}

test("onboarding prevents duplicate validation and stays on the API page if it fails", async () => {
  let release, validations = 0;
  const pending = new Promise((_, reject) => { release = () => reject(new Error("fixture key rejected")); });
  const { connect, calls } = fixture({ connectHarnessProvider: () => { validations++; return pending; } });
  const first = connect();
  await connect();
  assert.equal(validations, 1);
  release();
  await first;
  assert.equal(calls.some(call => call[0] === "step"), false);
  assert.ok(calls.some(call => call[0] === "error" && call[1]));
});
