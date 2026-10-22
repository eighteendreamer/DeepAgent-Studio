const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const ts = require("typescript");

const source = ts.createSourceFile("App.tsx", readFileSync(join(__dirname, "../src/App.tsx"), "utf8"),
  ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);

function callback(name, environment) {
  let body;
  function visit(node) {
    if (ts.isVariableDeclaration(node) && node.name.getText(source) === name) {
      assert.ok(ts.isCallExpression(node.initializer));
      body = node.initializer.arguments[0];
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
  assert.ok(body, `missing ${name}`);
  const code = ts.transpileModule(`exports.action = ${body.getText(source)};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const context = { exports: {}, ...environment };
  vm.runInNewContext(code, context);
  return context.exports.action;
}

function fixture() {
  const state = { open: true, focus: 0 };
  return { state, environment: {
    setIsSidebarOpen(next) { state.open = typeof next === "function" ? next(state.open) : next; },
    runUiTransition() { assert.fail("sidebar resizing must not create top-layer snapshots outside the rounded frame"); },
    requestAnimationFrame(fn) { fn(); },
    document: { querySelector(selector) {
      assert.equal(selector, '[data-sidebar-toggle="rail"]');
      return { focus() { state.focus++; } };
    } },
  } };
}

test("rapid sidebar toggles update synchronously without a page screenshot transition", () => {
  const { state, environment } = fixture();
  const toggle = callback("toggleLeftSidebar", environment);
  for (const expected of [false, true, false, true, false]) {
    toggle();
    assert.equal(state.open, expected);
  }
});

test("sidebar dismissal retains focus return without starting a page screenshot transition", () => {
  const { state, environment } = fixture();
  const dismiss = callback("dismissLeftSidebar", environment);
  dismiss(); dismiss();
  assert.equal(state.open, false);
  assert.equal(state.focus, 2);
});
