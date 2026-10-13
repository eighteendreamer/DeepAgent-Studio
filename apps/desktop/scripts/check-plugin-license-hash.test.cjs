const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { pluginDirectoryContentHash } = require("./check-plugin-licenses.cjs");

test("generated payload exclusion preserves the source integrity check", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "plugin-hash-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "README.md"), "original source");
  const excluded = new Set(["runtime.zip", "runtime.zip.json"]);
  const sourceHash = pluginDirectoryContentHash(root, excluded);
  fs.writeFileSync(path.join(root, "runtime.zip"), "generated output");
  fs.writeFileSync(path.join(root, "runtime.zip.json"), "generated metadata");
  assert.equal(pluginDirectoryContentHash(root, excluded), sourceHash);
  assert.notEqual(pluginDirectoryContentHash(root), sourceHash);
  fs.writeFileSync(path.join(root, "README.md"), "changed source");
  assert.notEqual(pluginDirectoryContentHash(root, excluded), sourceHash);
});
