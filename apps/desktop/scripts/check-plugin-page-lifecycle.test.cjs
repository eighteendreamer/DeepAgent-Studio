const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const source = readFileSync(join(__dirname, "..", "src", "App.tsx"), "utf8");

test("plugin page stays mounted after its first visit", () => {
  assert.match(
    source,
    /const \[pluginsMounted, setPluginsMounted\] = useState\(false\)/,
  );

  const start = source.indexOf('{(view === "plugins" || pluginsMounted) && (');
  const end = source.indexOf('{view === "automation" && (', start);
  assert.notEqual(start, -1, "plugin page must remain mounted after first visit");
  assert.notEqual(end, -1, "automation view must follow the plugin page");

  const implementation = source.slice(start, end);
  assert.match(
    implementation,
    /className=\{view === "plugins" \? "view-frame" : "hidden"\}/,
  );
  assert.doesNotMatch(implementation, /key=\{viewFrameKey\}/);
});
