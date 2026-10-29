const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const ts = require("typescript");

function load(relative, imports = {}) {
  const filename = path.resolve(__dirname, relative);
  const compiled = ts.transpileModule(readFileSync(filename, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
    fileName: filename,
  });
  const loaded = new Module(filename, module);
  loaded.filename = filename;
  loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  const original = loaded.require.bind(loaded);
  loaded.require = (name) => Object.hasOwn(imports, name) ? imports[name] : original(name);
  loaded._compile(compiled.outputText, filename);
  return loaded.exports;
}

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

async function fixture(writeOverride = async () => {}) {
  global.window = { __TAURI_INTERNALS__: {} };
  global.localStorage = { getItem: () => null, removeItem: () => {} };
  const writes = [];
  const invoke = async (command, args) => {
    if (command === "canvas_preference_read") return null;
    if (command === "canvas_preference_write") {
      writes.push(args);
      await writeOverride(args);
      return {};
    }
    throw new Error(`unexpected command ${command}`);
  };
  const preferences = load("../src/canvas/workflow/utils/canvasPreferences.ts", {
    "@tauri-apps/api/core": { invoke },
    "./canvasMedia": { importCanvasMedia: async () => "artifact://upgraded" },
  });
  const adapters = load("../src/canvas/workflow/utils/videoEditAdapters.ts", {
    "@tauri-apps/api/core": { invoke },
    "./canvasPreferences": preferences,
    "./canvasMedia": { importCanvasMedia: async () => "artifact://imported" },
  });
  await preferences.whenCanvasPreferencesReady();
  return { adapters, writes };
}

test("saving an edit project waits for the debounced preference write to finish", async () => {
  const barrier = deferred();
  const f = await fixture(() => barrier.promise);
  let settled = false;
  const saving = f.adapters.saveVideoEditProject({
    id: "project-1", inputRef: "artifact://source", duration: 8,
    trimStart: 1, trimEnd: 6, updatedAt: 0,
  }).then(() => { settled = true; });

  while (f.writes.length === 0) await new Promise((resolve) => setImmediate(resolve));
  assert.equal(f.writes.at(-1).key, "video-edit-projects");
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(settled, false, "save must not resolve while persistence is pending");

  barrier.resolve();
  await saving;
  assert.equal(settled, true);
});

test("saving an edit project reports database persistence failures", async () => {
  const f = await fixture(async () => { throw new Error("database unavailable"); });
  await assert.rejects(
    f.adapters.saveVideoEditProject({
      id: "project-2", inputRef: "artifact://source", duration: 8,
      trimStart: 0, trimEnd: 8, updatedAt: 0,
    }),
    /database unavailable/,
  );
});

test("rapid project saves share one debounced write without losing either project", async () => {
  const f = await fixture();
  await Promise.all([
    f.adapters.saveVideoEditProject({
      id: "project-a", inputRef: "artifact://a", duration: 8,
      trimStart: 0, trimEnd: 8, updatedAt: 0,
    }),
    f.adapters.saveVideoEditProject({
      id: "project-b", inputRef: "artifact://b", duration: 12,
      trimStart: 2, trimEnd: 10, updatedAt: 0,
    }),
  ]);
  const writes = f.writes.filter(({ key }) => key === "video-edit-projects");
  assert.equal(writes.length, 1, "rapid changes should stay within the shared debounce");
  assert.deepEqual(Object.keys(writes[0].value).sort(), ["project-a", "project-b"]);
});

test("recorded uploads freeze the explicit namespace across every IPC action", async () => {
  for (const workspace of [null, " ws-a ", "ws-b"]) {
    const calls = [];
    const adapters = load("../src/canvas/workflow/utils/videoEditAdapters.ts", {
      "./canvasPreferences": {},
      "@tauri-apps/api/core": { invoke: async (command, args) => {
        calls.push({ command, ...args });
        return args.action.action === "begin_streaming" ? "upload-test" : { uri: "artifact://owned" };
      } },
    });
    async function* chunks() { yield new Blob([new Uint8Array(256 * 1024 + 1)]); }
    assert.equal(await adapters.uploadRecordedVideo(chunks(), "video/webm", "synthetic.webm", workspace), "artifact://owned");
    assert.deepEqual(calls.map(call => call.action.action), ["begin_streaming", "write", "write", "finish"]);
    assert.ok(calls.every(call => call.command === "canvas_artifact_upload"));
    assert.deepEqual(calls.map(call => call.workspaceId), Array(4).fill(workspace?.trim() ?? null));
    assert.deepEqual(calls.filter(call => call.action.action === "write").map(call => call.action.offset), [0, 256 * 1024]);
  }
});

test("upload failure aborts only in its original namespace and preserves the error", async () => {
  for (const failedAction of ["write", "finish"]) {
    const calls = [];
    const adapters = load("../src/canvas/workflow/utils/videoEditAdapters.ts", {
      "./canvasPreferences": {},
      "@tauri-apps/api/core": { invoke: async (_command, args) => {
        calls.push(args);
        if (args.action.action === failedAction) throw Error("synthetic upload failure");
        if (args.action.action === "abort") throw Error("synthetic abort failure");
        return "upload-test";
      } },
    });
    async function* chunks() { yield new Blob(["synthetic"]); }
    await assert.rejects(adapters.uploadRecordedVideo(chunks(), "video/webm", "synthetic.webm", "ws-a"), /synthetic upload failure/);
    assert.equal(calls.at(-1).action.action, "abort");
    assert.ok(calls.every(call => call.workspaceId === "ws-a"));
  }
});
