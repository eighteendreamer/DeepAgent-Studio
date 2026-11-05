const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

// Execute the production composer-to-run mapping, including TS type erasure.
const source = fs.readFileSync(path.join(__dirname, "../src/App.tsx"), "utf8");
const start = source.indexOf("const runAttachments: RunAttachment[] =");
const end = source.indexOf("if (preflightController.signal.aborted)", start);
assert.ok(start >= 0 && end > start);
const code = ts.transpileModule(source.slice(start, end), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const project = new Function("nativeVision", "imageAttachments", `${code}; return runAttachments;`);

test("native image run input preserves the ingestion digest", () => {
  const image = { id: "fixture", originalPath: "synthetic.png", mime: "image/png", sha256: "ab".repeat(32) };
  assert.deepEqual(project(true, [image]), [{
    type: "image", id: image.id, path: image.originalPath, media_type: image.mime, sha256: image.sha256,
  }]);
  assert.deepEqual(project(false, [image]), []);
});

test("legacy composer state cannot synthesize a content identity", () => {
  assert.equal(project(true, [{ id: "old", originalPath: "old.png" }])[0].sha256, null);
  assert.deepEqual(project(true, [{ id: "missing" }]), []);
});
