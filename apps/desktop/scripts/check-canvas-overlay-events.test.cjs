const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync } = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../src/canvas/workflow/components");
const overlays = {
  "CreativeLibraryPanel.tsx": ["主创意库", "wf-floating-layer fixed inset-0 flex flex-col", "wf-floating-layer fixed inset-0 flex items-center justify-center"],
  "CropOverlay.tsx": ["裁剪弹窗", "wf-floating-layer fixed inset-0 flex flex-col"],
  "DrawingOverlay.tsx": ["涂鸦弹窗", "wf-floating-layer fixed inset-0 z-[10000]"],
  "OutpaintOverlay.tsx": ["扩图弹窗", "wf-floating-layer fixed inset-0 z-[10000]"],
  "NodeFloatingToolbar.tsx": ["工具栏子菜单", "className=\"wf-floating-layer overflow-hidden\""],
};

for (const [file, [label, ...markers]] of Object.entries(overlays)) {
  test(`${label} marks its portal root as a canvas floating layer`, () => {
    const source = readFileSync(path.join(root, file), "utf8");
    for (const marker of markers) assert.ok(source.includes(marker), `${file} 缺少 ${marker}`);
  });
}

test("node toolbar isolates both submenu portal levels", () => {
  const source = readFileSync(path.join(root, "NodeFloatingToolbar.tsx"), "utf8");
  const marker = 'className="wf-floating-layer overflow-hidden"';
  assert.equal(source.split(marker).length - 1, 2);
});

test("canvas double-click handler ignores floating layers", () => {
  const source = readFileSync(path.resolve(__dirname, "../src/canvas/CanvasApp.tsx"), "utf8");
  assert.match(source, /target\.closest\("\.react-flow__node, \.react-flow__edge, \.wf-floating-layer"\)/);
});
