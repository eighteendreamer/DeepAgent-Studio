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

// Execute the real picker/drop/paste handler, not a second upload implementation.
const composer = fs.readFileSync(path.join(__dirname, "../src/components/Composer.tsx"), "utf8");
const addStart = composer.indexOf("const addFiles = async");
const addEnd = composer.indexOf("const onPaste =", addStart);
assert.ok(addStart >= 0 && addEnd > addStart);
const handler = ts.transpileModule(composer.slice(addStart, addEnd), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const guardSource = fs.readFileSync(path.join(__dirname, "../src/components/composerImageInput.ts"), "utf8");
const guardModule = { exports: {} };
new Function("exports", ts.transpileModule(guardSource, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText)(guardModule.exports);

function uploads(backendFailure = false) {
  const reads = [], calls = [], ready = [], errors = [];
  let next = 0;
  const add = new Function("attachments", "MAX_COMPOSER_ATTACHMENTS", "showAttachmentLimitWarning",
    "attachmentId", "isTextLikeFile", "addAttachment", "readFileAsDataUrl", "readFileAsText",
    "attachmentIngest", "markAttachmentReady", "markAttachmentError", "validateComposerImage",
    `${handler}; return addFiles;`)([], 5, () => {}, () => `fixture-${next++}`,
    file => file.type.startsWith("text/"), () => {},
    async file => { reads.push(file); return "data:image/png;base64,fixture"; }, async () => "plain text",
    async value => { calls.push(value); if (backendFailure) throw new Error("image_input_invalid"); return { original_path: "synthetic", size_bytes: 1, sha256: "ab".repeat(32) }; },
    (...args) => ready.push(args), (...args) => errors.push(args), guardModule.exports.validateComposerImage);
  return { add, reads, calls, ready, errors };
}

test("H169 unsupported picker/drop/paste images show an error before reading or ingestion", async () => {
  for (const type of ["image/svg+xml", "image/bmp", "image/tiff", "image/avif"]) {
    for (const source of ["picker", "drop", "paste"]) {
      const ui = uploads();
      await ui.add([{ name: "synthetic.png", type, size: 10 }], source);
      assert.equal(ui.reads.length, 0);
      assert.equal(ui.calls.length, 0);
      assert.equal(ui.ready.length, 0);
      assert.equal(ui.errors.length, 1);
    }
  }
});

test("H169 four image MIME types and ordinary text retain the existing upload route", async () => {
  const ui = uploads();
  await ui.add(["image/png", "image/jpeg", "image/gif", "image/webp", "text/plain"]
    .map(type => ({ type, name: "synthetic.bin", size: 10 })), "picker");
  assert.equal(ui.calls.length, 5);
  assert.equal(ui.reads.length, 4);
  assert.equal(ui.ready.length, 5);
  assert.equal(ui.errors.length, 0);
});

test("H169 image byte limits are checked before reading, and backend decode errors stay visible", async () => {
  for (const size of [0, 32*1024*1024+1]) {
    const ui = uploads();
    await ui.add([{ type: "image/png", name: "synthetic.png", size }], "paste");
    assert.equal(ui.reads.length, 0);
    assert.equal(ui.calls.length, 0);
    assert.equal(ui.errors.length, 1);
  }
  const ui = uploads(true);
  await ui.add([{ type: "image/png", name: "claimed.png", size: 8 }], "picker");
  assert.equal(ui.calls.length, 1);
  assert.equal(ui.ready.length, 0);
  assert.equal(ui.errors.length, 1);
  assert.match(ui.errors[0][1].message, /image_input_invalid/);
});
