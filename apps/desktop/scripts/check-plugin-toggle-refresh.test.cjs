const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const source = readFileSync(
  join(__dirname, "..", "src", "components", "PluginsViewReal.tsx"),
  "utf8",
);

test("plugin toggle refreshes plugin state without a full page reload", () => {
  const start = source.indexOf("const applyPluginToggle = async");
  const end = source.indexOf("const togglePlugin = async", start);

  assert.notEqual(start, -1, "applyPluginToggle must exist");
  assert.notEqual(end, -1, "togglePlugin must follow applyPluginToggle");

  const implementation = source.slice(start, end);
  assert.doesNotMatch(implementation, /\bload\(\)/);
  assert.match(implementation, /setPlugins\(/);
  assert.match(implementation, /listPlugins\(\)/);
  assert.match(implementation, /listPluginOutputStyles\(\)/);
  assert.doesNotMatch(
    implementation,
    /listPluginMarketplaces|listPluginMarketplaceEntries/,
  );
});
