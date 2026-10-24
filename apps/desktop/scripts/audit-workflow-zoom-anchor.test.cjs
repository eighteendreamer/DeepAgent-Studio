const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const { test } = require("node:test");
const ts = require("typescript");

function loadProductionHandler(relativePath, variableName, scope) {
  const filename = path.resolve(__dirname, relativePath);
  const source = readFileSync(filename, "utf8");
  const ast = ts.createSourceFile(filename, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let initializer;
  const visit = (node) => {
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === variableName) {
      initializer = node.initializer;
    }
    ts.forEachChild(node, visit);
  };
  visit(ast);
  assert.ok(initializer && ts.isArrowFunction(initializer), `missing production handler ${variableName}`);

  const compiled = ts.transpileModule(
    `module.exports = (${initializer.getText(ast)});`,
    { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } },
  ).outputText;
  const loaded = { exports: {} };
  new Function("module", "exports", ...Object.keys(scope), compiled)(
    loaded,
    loaded.exports,
    ...Object.values(scope),
  );
  return loaded.exports;
}

const WIDTH = 1000;
const HEIGHT = 600;
const START = { x: -500, y: -240, zoom: 1 };

function viewportCenterInWorld(viewport) {
  return {
    x: (WIDTH / 2 - viewport.x) / viewport.zoom,
    y: (HEIGHT / 2 - viewport.y) / viewport.zoom,
  };
}

function runBottomBarHandler(name, args = [], viewport = START) {
  let nextViewport;
  const handler = loadProductionHandler(
    "../src/canvas/workflow/components/BottomBar.tsx",
    name,
    {
      zoom: viewport.zoom,
      viewport,
      onViewportChange: (viewport) => { nextViewport = viewport; },
      rfInstance: { setViewport: (viewport) => { nextViewport = viewport; } },
    },
  );
  handler(...args);
  assert.ok(nextViewport, `${name} did not request a viewport update`);
  return nextViewport;
}

function runMiniMapZoom(delta) {
  let nextViewport;
  const handler = loadProductionHandler(
    "../src/canvas/workflow/components/MiniMap.tsx",
    "changeZoom",
    {
      getViewport: () => START,
      setFlowViewport: (viewport) => { nextViewport = viewport; },
    },
  );
  handler(delta);
  assert.ok(nextViewport, "minimap zoom did not request a viewport update");
  return nextViewport;
}

test("workflow zoom controls preserve the visible world center", {
  skip: process.env.RUN_HARNESS_AUDIT !== "1",
}, () => {
  const resetStart = { x: -650, y: -360, zoom: 1.5 };
  const viewports = [
    ["bottom bar zoom in", START, runBottomBarHandler("handleZoomIn")],
    ["bottom bar zoom out", START, runBottomBarHandler("handleZoomOut")],
    ["bottom bar slider", START, runBottomBarHandler("handleSlider", [{ target: { value: "110" } }])],
    ["bottom bar reset", resetStart, runBottomBarHandler("handleResetZoom", [], resetStart)],
    ["minimap zoom in", START, runMiniMapZoom(0.1)],
    ["minimap zoom out", START, runMiniMapZoom(-0.1)],
  ];
  const observations = viewports.map(([control, before, viewport]) => {
    const expectedCenter = viewportCenterInWorld(before);
    return {
      control,
      viewport,
      centerBefore: expectedCenter,
      centerAfter: viewportCenterInWorld(viewport),
    };
  });
  console.log(JSON.stringify(observations));

  assert.ok(
    observations.every(({ centerBefore, centerAfter }) =>
      Math.abs(centerAfter.x - centerBefore.x) < 1e-9
      && Math.abs(centerAfter.y - centerBefore.y) < 1e-9),
    "zoom changed the visible world center; zoom should preserve the current focal point",
  );
});
