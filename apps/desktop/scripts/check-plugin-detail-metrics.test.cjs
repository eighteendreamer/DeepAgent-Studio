const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const source = readFileSync(
  join(__dirname, "..", "src", "components", "PluginsViewReal.tsx"),
  "utf8",
);

test("plugin detail metrics use fixed compact cards", () => {
  assert.match(source, /className="mb-8 flex flex-wrap gap-2"/);
  for (const label of ["技能", "MCP", "钩子", "命令", "应用", "样式"]) {
    assert.match(source, new RegExp(`Metric label="${label}"`));
  }
  assert.match(source, /h-14 w-\[102px\] shrink-0 rounded-md/);
});
