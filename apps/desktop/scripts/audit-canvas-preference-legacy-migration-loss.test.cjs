const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const { test } = require("node:test");
const ts = require("typescript");

function loadTypeScript(relativePath, imports = {}) {
  const filename = path.resolve(__dirname, relativePath);
  const compiled = ts.transpileModule(readFileSync(filename, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
    fileName: filename,
  });
  const loaded = new Module(filename, module);
  loaded.filename = filename;
  loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  const requireModule = loaded.require.bind(loaded);
  loaded.require = (specifier) =>
    Object.hasOwn(imports, specifier) ? imports[specifier] : requireModule(specifier);
  loaded._compile(compiled.outputText, filename);
  return loaded.exports;
}

const workspaceKey = "workflow-workspace";
const workspaceValue = { imageDir: "legacy-images", videoDir: "legacy-videos" };
const workspaceRaw = JSON.stringify(workspaceValue);
const tick = () => new Promise((resolve) => setImmediate(resolve));

function fixture(t, { legacy = new Map([[workspaceKey, workspaceRaw]]), persisted = new Map(), before = async () => {} } = {}) {
  const previousWindow = globalThis.window;
  const previousLocalStorage = globalThis.localStorage;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const storageFaults = { read: false, remove: false };
  const operations = [], calls = [], imports = [], commits = [];
  globalThis.localStorage = {
    getItem: (key) => {
      if (storageFaults.read && key === workspaceKey) throw new Error("storage unavailable");
      return legacy.get(key) ?? null;
    },
    removeItem: (key) => {
      if (storageFaults.remove) throw new Error("cleanup unavailable");
      operations.push({ type: "remove", key });
      legacy.delete(key);
    },
  };
  const invoke = async (command, args) => {
    calls.push({ command, ...structuredClone(args) });
    if (command === "canvas_artifact_import") imports.push(args.artifact.dataUrl);
    await before(command, args);
    if (command === "canvas_preference_read") return structuredClone(persisted.get(args.key) ?? null);
    if (command === "canvas_preference_write") {
      const value = structuredClone(args.value);
      persisted.set(args.key, value);
      commits.push({ key: args.key, value });
      operations.push({ type: "commit", key: args.key });
      return 1;
    }
    if (command === "canvas_artifact_import") return { uri: `artifact://synthetic-${imports.length}` };
    throw new Error(`unexpected Tauri command: ${command}`);
  };
  const load = () => {
    // Exercise the real media adapter too; only IPC/storage are controlled.
    const media = loadTypeScript("../src/canvas/workflow/utils/canvasMedia.ts", {
      "@tauri-apps/api/core": { invoke },
    });
    return loadTypeScript("../src/canvas/workflow/utils/canvasPreferences.ts", {
      "@tauri-apps/api/core": { invoke }, "./canvasMedia": media,
    });
  };
  t.after(() => {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
    if (previousLocalStorage === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = previousLocalStorage;
  });
  return { preferences: load(), load, legacy, persisted, calls, commits, imports, operations, storageFaults };
}

test("legacy canvas preference must survive a failed migration write", async (t) => {
  let fail = true;
  const f = fixture(t, { before: async (command) => {
    if (fail && command === "canvas_preference_write") throw new Error("simulated migration write failure");
  } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /simulated migration write failure/);
  console.log(JSON.stringify({ legacyAfter: f.legacy.get(workspaceKey), cacheLoaded: f.preferences.hasCanvasPreference("workspace-dirs") }));
  assert.equal(f.legacy.get(workspaceKey), workspaceRaw, "failed migration must retain the legacy source for retry");
  assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), false);
  assert.equal(f.persisted.has("workspace-dirs"), false);
  assert.throws(() => f.preferences.writeCanvasPreference("workspace-dirs", {}), /尚未成功读取/);
  fail = false;
  await f.preferences.whenCanvasPreferencesReady();
  assert.equal(f.legacy.has(workspaceKey), false);
  assert.deepEqual(f.persisted.get("workspace-dirs"), workspaceValue);
  assert.deepEqual(f.preferences.readCanvasPreference("workspace-dirs", null), workspaceValue);
  assert.deepEqual(f.operations, [{ type: "commit", key: "workspace-dirs" }, { type: "remove", key: workspaceKey }]);
  const reopened = f.load();
  await reopened.whenCanvasPreferencesReady();
  assert.deepEqual(reopened.readCanvasPreference("workspace-dirs", null), workspaceValue);
  assert.equal(f.commits.length, 1, "repeated startup reads the committed document without migrating again");
});

test("an in-flight migration retains its source until acknowledgement", async (t) => {
  let release;
  const barrier = new Promise((resolve) => { release = resolve; });
  const f = fixture(t, { before: (command) => command === "canvas_preference_write" ? barrier : undefined });
  const ready = f.preferences.whenCanvasPreferencesReady();
  await tick();
  assert.equal(f.legacy.get(workspaceKey), workspaceRaw);
  assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), false);
  assert.deepEqual(f.operations, []);
  release();
  await ready;
  assert.deepEqual(f.operations.map((op) => op.type), ["commit", "remove"]);
});

test("one failed key does not discard other migrations or repeat their writes", async (t) => {
  let fail = true;
  const f = fixture(t, {
    legacy: new Map([[workspaceKey, workspaceRaw], ["canvas-creative-library", '[{"id":"keep"}]'], ["workflow-canvas-professional-snippets", '{"version":1,"snippets":[]}']]),
    before: async (command, args) => {
      if (command === "canvas_preference_write" && args.key === "workspace-dirs" && fail) throw new Error("workspace write failed");
    },
  });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /workspace write failed/);
  assert.deepEqual([...f.legacy.keys()], [workspaceKey]);
  assert.equal(f.preferences.hasCanvasPreference("creative-library"), true);
  assert.equal(f.preferences.hasCanvasPreference("workflow-snippets"), true);
  assert.equal(f.preferences.hasCanvasPreference("video-edit-projects"), true);
  fail = false;
  await f.preferences.whenCanvasPreferencesReady();
  assert.equal(f.legacy.size, 0);
  assert.deepEqual(f.commits.map(({ key }) => key), ["creative-library", "workflow-snippets", "workspace-dirs"]);
});

test("corrupt or inaccessible legacy data is not empty and cannot authorize fallback writes", async (t) => {
  const damaged = '{"private-content":"synthetic-sensitive-fragment"';
  const f = fixture(t, { legacy: new Map([[workspaceKey, damaged]]) });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), (error) => {
    assert.match(error.message, /workspace-dirs JSON 无效/);
    assert.ok(!error.message.includes("synthetic-sensitive-fragment"), "parse errors must not echo legacy content");
    return true;
  });
  assert.equal(f.legacy.get(workspaceKey), damaged);
  assert.equal(f.commits.length, 0);
  f.storageFaults.read = true;
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /无法读取旧画布偏好/);
  assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), false);
  assert.equal(f.legacy.get(workspaceKey), damaged);
  f.storageFaults.read = false;
  f.legacy.set(workspaceKey, workspaceRaw);
  await f.preferences.whenCanvasPreferencesReady();
  assert.deepEqual(f.persisted.get("workspace-dirs"), workspaceValue);
});

for (const raw of ["null", "false", "0", '""']) {
  test(`valid legacy JSON ${raw} is distinct from an absent storage key`, async (t) => {
    const f = fixture(t, { legacy: new Map([[workspaceKey, raw]]) });
    await f.preferences.whenCanvasPreferencesReady();
    assert.equal(f.persisted.has("workspace-dirs"), true);
    assert.deepEqual(f.persisted.get("workspace-dirs"), JSON.parse(raw));
    assert.equal(f.legacy.has(workspaceKey), false);
    assert.equal(f.commits.length, 1);
  });
}

test("cleanup failure preserves both copies without making the next startup overwrite the database", async (t) => {
  const f = fixture(t);
  f.storageFaults.remove = true;
  await f.preferences.whenCanvasPreferencesReady();
  assert.equal(f.legacy.get(workspaceKey), workspaceRaw);
  assert.deepEqual(f.persisted.get("workspace-dirs"), workspaceValue);
  assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), true);
  f.persisted.set("workspace-dirs", { ...workspaceValue, imageDir: "newer-database" });
  f.storageFaults.remove = false;
  const reopened = f.load();
  await reopened.whenCanvasPreferencesReady();
  assert.equal(reopened.readCanvasPreference("workspace-dirs", null).imageDir, "newer-database");
  assert.equal(f.legacy.get(workspaceKey), workspaceRaw, "pre-existing database contents do not prove ownership of a legacy source");
  assert.equal(f.commits.length, 1);
});

test("a lost write response retains legacy and recovers from the committed backend without replaying the write", async (t) => {
  const persisted = new Map();
  const f = fixture(t, { persisted, before: async (command, args) => {
    if (command === "canvas_preference_write") {
      persisted.set(args.key, structuredClone(args.value));
      throw new Error("response lost after commit");
    }
  } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /response lost after commit/);
  assert.equal(f.legacy.get(workspaceKey), workspaceRaw);
  await f.preferences.whenCanvasPreferencesReady();
  assert.deepEqual(f.preferences.readCanvasPreference("workspace-dirs", null), workspaceValue);
  const reopened = f.load();
  await reopened.whenCanvasPreferencesReady();
  assert.deepEqual(reopened.readCanvasPreference("workspace-dirs", null), workspaceValue);
  assert.equal(f.calls.filter(({ command }) => command === "canvas_preference_write").length, 1);
  assert.equal(f.legacy.get(workspaceKey), workspaceRaw, "unknown acknowledgement cannot authorize source cleanup");
});

test("restart after an uncommitted write failure can still migrate the original source", async (t) => {
  let fail = true;
  const f = fixture(t, { before: async (command) => {
    if (command === "canvas_preference_write" && fail) throw new Error("not committed");
  } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /not committed/);
  fail = false;
  const reopened = f.load();
  await reopened.whenCanvasPreferencesReady();
  assert.deepEqual(reopened.readCanvasPreference("workspace-dirs", null), workspaceValue);
  assert.equal(f.legacy.has(workspaceKey), false);
  assert.equal(f.commits.length, 1);
});

test("cleanup cannot erase a legacy snapshot replaced while the migration write is pending", async (t) => {
  const legacy = new Map([[workspaceKey, workspaceRaw]]);
  const newer = JSON.stringify({ ...workspaceValue, imageDir: "new-legacy" });
  const f = fixture(t, { legacy, before: async (command) => {
    if (command === "canvas_preference_write") legacy.set(workspaceKey, newer);
  } });
  await f.preferences.whenCanvasPreferencesReady();
  assert.equal(legacy.get(workspaceKey), newer);
  assert.deepEqual(f.persisted.get("workspace-dirs"), workspaceValue);
  assert.deepEqual(f.operations, [{ type: "commit", key: "workspace-dirs" }]);
});

test("a changed source invalidates an earlier uncommitted migration candidate", async (t) => {
  let fail = true;
  const f = fixture(t, { before: async (command) => {
    if (command === "canvas_preference_write" && fail) throw new Error("retry");
  } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /retry/);
  const newer = { ...workspaceValue, videoDir: "new-legacy-videos" };
  f.legacy.set(workspaceKey, JSON.stringify(newer));
  fail = false;
  await f.preferences.whenCanvasPreferencesReady();
  assert.deepEqual(f.persisted.get("workspace-dirs"), newer);
  assert.equal(f.legacy.has(workspaceKey), false);
});

test("partial image imports retain every entry and reuse prepared results after a preference write failure", async (t) => {
  const first = "data:image/png;base64,YQ==", second = "data:image/png;base64,Yg==";
  const items = [{ id: "a", imageUrl: first, name: "keep-a" }, { id: "b", imageUrl: second, name: "keep-b" }, { id: "text", prompt: "keep-text" }];
  const raw = JSON.stringify(items);
  let failWrite = true;
  const f = fixture(t, { legacy: new Map([["canvas-creative-library", raw]]), before: async (command, args) => {
    if (command === "canvas_artifact_import" && args.artifact.dataUrl === second) throw new Error("second import failed");
    if (command === "canvas_preference_write" && failWrite) throw new Error("library write failed");
  } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /library write failed/);
  assert.equal(f.legacy.get("canvas-creative-library"), raw);
  assert.equal(f.preferences.hasCanvasPreference("creative-library"), false);
  assert.deepEqual(f.imports, [first, second]);
  failWrite = false;
  await f.preferences.whenCanvasPreferencesReady();
  assert.deepEqual(f.imports, [first, second], "retry of the same prepared source must not repeat artifact imports");
  const expected = [{ ...items[0], imageUrl: "artifact://synthetic-1" }, items[1], items[2]];
  assert.deepEqual(f.persisted.get("creative-library"), expected, "failed imports retain their original bytes, not an empty entry");
  assert.equal(f.legacy.has("canvas-creative-library"), false);
  const reopened = f.load();
  await reopened.whenCanvasPreferencesReady();
  assert.deepEqual(reopened.readCanvasPreference("creative-library", null), expected);
  assert.deepEqual(f.imports, [first, second]);
});
