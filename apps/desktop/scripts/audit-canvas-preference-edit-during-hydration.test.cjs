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

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

const tick = () => new Promise((resolve) => setImmediate(resolve));
const oldWorkspace = { imageDir: "old-images", videoDir: "old-videos" };

function fixture(t, { persisted = { "workspace-dirs": { ...oldWorkspace } }, read = async () => {}, write = async () => {}, legacy = new Map() } = {}) {
  const previousWindow = globalThis.window;
  const previousLocalStorage = globalThis.localStorage;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  globalThis.localStorage = {
    getItem: (key) => legacy.get(key) ?? null,
    removeItem: (key) => legacy.delete(key),
  };

  const writes = [], reads = [], changes = [];
  const invoke = async (command, args) => {
    if (command === "canvas_preference_read") {
      reads.push(args.key);
      // Capture a genuine old snapshot before waiting, rather than reading
      // the latest fixture value when the delayed response is released.
      const snapshot = structuredClone(persisted[args.key] ?? null);
      await read(args.key);
      return snapshot;
    }
    if (command === "canvas_preference_write") {
      await write(args.key);
      writes.push({ key: args.key, value: structuredClone(args.value) });
      persisted[args.key] = structuredClone(args.value);
      return 1;
    }
    if (command === "canvas_settings_read") {
      return { providers: [], bindings: [] };
    }
    throw new Error(`unexpected Tauri command: ${command}`);
  };

  const preferences = loadTypeScript("../src/canvas/workflow/utils/canvasPreferences.ts", {
    "@tauri-apps/api/core": { invoke },
    "./canvasMedia": { importCanvasMedia: async () => "artifact://audit" },
  });
  const settings = loadTypeScript("../src/canvas/workflow/store/canvasSettingsStore.ts", {
    "@tauri-apps/api/core": { invoke },
    "../utils/canvasPreferences": preferences,
  }).useCanvasSettingsStore;
  const unsubscribe = preferences.onCanvasPreferenceChanged((key) => {
    if (key === "workspace-dirs") changes.push(structuredClone(preferences.readCanvasPreference(key, null)));
  });
  t.after(async () => {
    unsubscribe();
    await preferences.flushCanvasPreferences().catch(() => {});
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
    if (previousLocalStorage === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = previousLocalStorage;
  });
  return { preferences, settings, writes, reads, changes, persisted, legacy };
}

function assertWorkspace(f, value) {
  assert.deepEqual(f.settings.getState().workspace, value);
  assert.deepEqual(f.preferences.readCanvasPreference("workspace-dirs", null), value);
}

test("late workspace read cannot accept a premature edit or erase a subsequent accepted edit", async (t) => {
  const barrier = deferred();
  const f = fixture(t, { read: (key) => key === "workspace-dirs" ? barrier.promise : undefined });
  const ready = f.preferences.whenCanvasPreferencesReady();
  try {
    await tick();
    // H-245 removed the original probe's 'early edit accepted' precondition.
    // Verify explicit rejection, then keep the original intended final value
    // by editing through the same public mutator once the baseline is ready.
    f.settings.getState().setWorkspaceDir("imageDir", "new-images");
    assert.match(f.settings.getState().lastError, /workspace-dirs.*尚未成功读取/);
    assert.deepEqual(f.settings.getState().workspace, { imageDir: "", videoDir: "" });
    assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), false);
    assert.deepEqual(f.changes, []);
    let flushed = false;
    const flushing = f.preferences.flushCanvasPreferences().then(() => { flushed = true; });
    await tick();
    assert.equal(flushed, false);
    assert.deepEqual(f.writes, []);
    barrier.resolve();
    await Promise.all([ready, flushing]);
    assertWorkspace(f, oldWorkspace);
    assert.deepEqual(f.writes, [], "rejected edits must not leave a queued fallback write");
    f.settings.getState().setWorkspaceDir("imageDir", "new-images");
    await f.preferences.flushCanvasPreferences();
    const expected = { imageDir: "new-images", videoDir: "old-videos" };
    assertWorkspace(f, expected);
    assert.deepEqual(f.writes, [{ key: "workspace-dirs", value: expected }]);
    assert.deepEqual(f.persisted["workspace-dirs"], expected);
    assert.deepEqual(f.changes, [oldWorkspace, expected]);
    assert.equal(f.settings.getState().lastError, null);
    await f.preferences.whenCanvasPreferencesReady();
    assert.equal(f.reads.filter((key) => key === "workspace-dirs").length, 1);
    console.log(JSON.stringify({ prematureEditRejected: true, stateAfterHydration: oldWorkspace, acceptedEdit: expected, writePayload: f.writes[0].value }));
  } finally {
    barrier.resolve();
    await ready;
  }
});

test("consecutive edits after a successful read preserve both fields and commit the latest value", async (t) => {
  const f = fixture(t);
  await f.preferences.whenCanvasPreferencesReady();
  f.settings.getState().setWorkspaceDir("imageDir", "first-images");
  f.settings.getState().setWorkspaceDir("imageDir", "latest-images");
  f.settings.getState().setWorkspaceDir("videoDir", "latest-videos");
  await f.preferences.flushCanvasPreferences();
  const expected = { imageDir: "latest-images", videoDir: "latest-videos" };
  assertWorkspace(f, expected);
  assert.deepEqual(f.writes, [{ key: "workspace-dirs", value: expected }]);
  assert.deepEqual(f.persisted["workspace-dirs"], expected);
  assert.equal(f.changes.length, 4, "one baseline plus three accepted edits, no late snapshot publish");
});

test("an accepted workspace edit survives completion of a different pending preference read", async (t) => {
  const barrier = deferred();
  const f = fixture(t, { read: (key) => key === "video-edit-projects" ? barrier.promise : undefined });
  const ready = f.preferences.whenCanvasPreferencesReady();
  try {
    await tick();
    assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), true);
    assert.equal(f.preferences.hasCanvasPreference("video-edit-projects"), false);
    f.settings.getState().setWorkspaceDir("imageDir", "accepted-images");
    f.settings.getState().setWorkspaceDir("videoDir", "accepted-videos");
    assert.equal(f.preferences.whenCanvasPreferencesReady(), ready);
    barrier.resolve();
    await ready;
    await f.preferences.flushCanvasPreferences();
    const expected = { imageDir: "accepted-images", videoDir: "accepted-videos" };
    assertWorkspace(f, expected);
    assert.deepEqual(f.persisted["workspace-dirs"], expected);
    assert.deepEqual(f.writes, [{ key: "workspace-dirs", value: expected }]);
    assert.equal(f.reads.filter((key) => key === "workspace-dirs").length, 1);
    assert.equal(f.changes.length, 3);
  } finally { barrier.resolve(); await ready; }
});

test("a delayed retry rejects unread workspace edits and permits normal editing after recovery", async (t) => {
  const barrier = deferred();
  let attempts = 0;
  const f = fixture(t, { read: (key) => {
    if (key !== "workspace-dirs") return;
    if (++attempts === 1) throw new Error("synthetic read failure");
    return barrier.promise;
  } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /synthetic read failure/);
  const retry = f.preferences.whenCanvasPreferencesReady();
  try {
    f.settings.getState().setWorkspaceDir("imageDir", "rejected-images");
    assert.match(f.settings.getState().lastError, /尚未成功读取/);
    assert.deepEqual(f.writes, []);
    assert.deepEqual(f.changes, []);
    barrier.resolve();
    await retry;
    assertWorkspace(f, oldWorkspace);
    f.settings.getState().setWorkspaceDir("videoDir", "recovered-videos");
    await f.preferences.flushCanvasPreferences();
    const expected = { imageDir: "old-images", videoDir: "recovered-videos" };
    assertWorkspace(f, expected);
    assert.deepEqual(f.persisted["workspace-dirs"], expected);
    assert.deepEqual(f.changes, [oldWorkspace, expected]);
    assert.equal(attempts, 2);
  } finally { barrier.resolve(); await retry; }
});

test("retrying a different failed key never reloads a workspace baseline over accepted edits", async (t) => {
  let fail = true;
  const f = fixture(t, { read: (key) => {
    if (key === "video-edit-projects" && fail) throw new Error("other key unavailable");
  } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /other key unavailable/);
  f.settings.getState().setWorkspaceDir("imageDir", "kept-images");
  fail = false;
  await f.preferences.whenCanvasPreferencesReady();
  await f.preferences.flushCanvasPreferences();
  const expected = { imageDir: "kept-images", videoDir: "old-videos" };
  assertWorkspace(f, expected);
  assert.deepEqual(f.persisted["workspace-dirs"], expected);
  assert.deepEqual(f.changes, [oldWorkspace, expected]);
  assert.equal(f.reads.filter((key) => key === "workspace-dirs").length, 1);
  assert.equal(f.reads.filter((key) => key === "video-edit-projects").length, 2);
});

test("legacy migration cannot authorize workspace edits until its write is acknowledged", async (t) => {
  const barrier = deferred();
  const f = fixture(t, {
    persisted: {}, legacy: new Map([["workflow-workspace", JSON.stringify(oldWorkspace)]]),
    write: () => barrier.promise,
  });
  const ready = f.preferences.whenCanvasPreferencesReady();
  try {
    await tick();
    f.settings.getState().setWorkspaceDir("imageDir", "premature-images");
    assert.match(f.settings.getState().lastError, /尚未成功读取/);
    assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), false);
    assert.deepEqual(f.changes, []);
    barrier.resolve();
    await ready;
    assertWorkspace(f, oldWorkspace);
    f.settings.getState().setWorkspaceDir("imageDir", "post-migration-images");
    await f.preferences.flushCanvasPreferences();
    const expected = { imageDir: "post-migration-images", videoDir: "old-videos" };
    assertWorkspace(f, expected);
    assert.deepEqual(f.persisted["workspace-dirs"], expected);
    assert.deepEqual(f.writes, [{ key: "workspace-dirs", value: oldWorkspace }, { key: "workspace-dirs", value: expected }]);
    assert.deepEqual(f.changes, [oldWorkspace, expected]);
  } finally { barrier.resolve(); await ready; }
});
