const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

const source = fs.readFileSync(path.join(__dirname, "../src/components/ui/virtualListLayout.ts"), "utf8");
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
});
const loaded = { exports: {} };
new Function("exports", "module", outputText)(loaded.exports, loaded);
const { virtualListOffsets, virtualListRange } = loaded.exports;

test("270 models mount only the visible range plus overscan", () => {
  const keys = Array.from({ length: 270 }, (_, index) => `model-${index}`);
  const offsets = virtualListOffsets(keys, new Map(), 68);
  const range = virtualListRange(offsets, 0, 600, 204);
  assert.equal(offsets.at(-1), 270 * 68);
  assert.equal(range.start, 0);
  assert.equal(range.end, 12);
  const bottom = virtualListRange(offsets, 1e6, 600, 204);
  assert.equal(bottom.end, 270);
  assert.equal(bottom.scrollTop, 270 * 68 - 600);
});

test("measured multi-line rows have contiguous offsets and cover the viewport", () => {
  const offsets = virtualListOffsets(["a", "b", "c", "d"], new Map([["a", 90], ["c", 120]]), 60);
  assert.deepEqual(offsets, [0, 90, 150, 270, 330]);
  const range = virtualListRange(offsets, 100, 100, 0);
  assert.equal(range.start, 1);
  assert.equal(range.end, 3);
  assert.ok(offsets[range.start] <= 100);
  assert.ok(offsets[range.end] >= 200);
});

test("filtering or removing models at the bottom clamps to remaining rows", () => {
  assert.deepEqual(virtualListRange([0, 68, 136], 17000, 600, 204), { start: 0, end: 2, scrollTop: 0 });
  assert.deepEqual(virtualListRange([0], 17000, 600, 204), { start: 0, end: 0, scrollTop: 0 });
  const offsets = virtualListOffsets(Array.from({ length: 270 }, (_, i) => String(i)), new Map(), 68);
  assert.equal(virtualListRange(offsets, 4200, 600, 204).scrollTop, 4200);
  assert.equal(virtualListRange(offsets, 17000, 3000, 204).scrollTop, 15360);
});
