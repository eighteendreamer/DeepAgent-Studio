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
  loaded.require = (specifier) => Object.hasOwn(imports, specifier) ? imports[specifier] : requireModule(specifier);
  loaded._compile(compiled.outputText, filename);
  return loaded.exports;
}

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

const tick = () => new Promise((resolve) => setImmediate(resolve));

function fixture(t, { persisted = {}, read = async () => {}, tauri = true } = {}) {
  const previousWindow = globalThis.window;
  const previousLocalStorage = globalThis.localStorage;
  globalThis.window = tauri ? { __TAURI_INTERNALS__: {} } : {};
  globalThis.localStorage = {
    getItem: () => null,
    removeItem: () => {},
  };

  const writes = [];
  const reads = [];
  const invoke = async (command, args) => {
    if (command === "canvas_preference_read") {
      reads.push(args.key);
      const result = await read(args.key);
      return structuredClone(result === undefined ? persisted[args.key] ?? null : result);
    }
    if (command === "canvas_preference_write") {
      writes.push({ key: args.key, value: structuredClone(args.value) });
      persisted[args.key] = structuredClone(args.value);
      return {};
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
  t.after(async () => {
    await preferences.flushCanvasPreferences().catch(() => {});
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
    if (previousLocalStorage === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = previousLocalStorage;
  });
  return { preferences, settings, writes, reads, persisted };
}

test("canvas preference read failure must not authorize a destructive fallback write", async (t) => {
  let failRead = true;
  const f = fixture(t, {
    persisted: { "workspace-dirs": { imageDir: "old-images", videoDir: "old-videos" } },
    read: async (key) => {
      if (key === "workspace-dirs" && failRead) throw new Error("simulated database read failure");
    },
  });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /workspace-dirs.*simulated database read failure/);
  await f.settings.getState().loadFromBackend();
  assert.equal(f.preferences.hasCanvasPreference("workspace-dirs"), false);
  f.settings.getState().setWorkspaceDir("imageDir", "new-images");
  assert.match(f.settings.getState().lastError, /workspace-dirs.*尚未成功读取/);
  assert.deepEqual(f.settings.getState().workspace, { imageDir: "", videoDir: "" }, "rejected edits must not appear accepted");
  await assert.rejects(f.preferences.flushCanvasPreferences(), /尚未成功读取/);
  assert.deepEqual(f.writes, []);
  assert.deepEqual(f.persisted["workspace-dirs"], { imageDir: "old-images", videoDir: "old-videos" });

  // Do not stop at rejection: recover the same production modules, then edit
  // through the original public mutator and check the actual persisted payload.
  failRead = false;
  await f.preferences.whenCanvasPreferencesReady();
  assert.deepEqual(f.settings.getState().workspace, { imageDir: "old-images", videoDir: "old-videos" });
  f.settings.getState().setWorkspaceDir("imageDir", "new-images");
  await f.preferences.flushCanvasPreferences();
  const write = f.writes.find((entry) => entry.key === "workspace-dirs");
  assert.deepEqual(write?.value, { imageDir: "new-images", videoDir: "old-videos" });
  assert.deepEqual(f.persisted["workspace-dirs"], write.value);
  assert.equal(f.settings.getState().lastError, null);
  console.log(JSON.stringify({ readFailureRejected: true, rejectedWrites: 0, recoveredPayload: write.value }));
});

test("all preference keys reject both write APIs before a read baseline exists", async (t) => {
  let failRead = true;
  const f = fixture(t, { read: async () => { if (failRead) throw new Error("offline"); } });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /offline/);
  for (const key of f.preferences.CANVAS_PREFERENCE_KEYS) {
    assert.throws(() => f.preferences.writeCanvasPreference(key, { unsafe: true }), /尚未成功读取/);
    // The Promise API must reject, not throw before returning a Promise.
    const saving = f.preferences.writeCanvasPreferenceAndWait(key, { unsafe: true });
    assert.ok(saving instanceof Promise);
    await assert.rejects(saving, /尚未成功读取/);
    assert.equal(f.preferences.hasCanvasPreference(key), false);
    assert.equal(f.preferences.readCanvasPreference(key, "fallback"), "fallback");
  }
  await assert.rejects(f.preferences.flushCanvasPreferences(), /尚未成功读取/);
  assert.deepEqual(f.writes, []);
  failRead = false;
  await f.preferences.whenCanvasPreferencesReady();
});

test("pending reads are not writable; repeated callers share hydration and flush waits", async (t) => {
  const barrier = deferred();
  const f = fixture(t, { read: () => barrier.promise });
  const first = f.preferences.whenCanvasPreferencesReady();
  assert.equal(first, f.preferences.whenCanvasPreferencesReady());
  assert.throws(() => f.preferences.writeCanvasPreference("workspace-dirs", {}), /尚未成功读取/);
  let flushed = false;
  const flushing = f.preferences.flushCanvasPreferences().then(() => { flushed = true; });
  await tick();
  assert.equal(flushed, false);
  barrier.resolve();
  await Promise.all([first, flushing]);
  assert.deepEqual(f.reads, f.preferences.CANVAS_PREFERENCE_KEYS);
  assert.deepEqual(f.writes, []);
});

test("retry reads only failed keys and cannot replace already accepted dirty edits", async (t) => {
  let failRead = true;
  const f = fixture(t, {
    persisted: { "creative-library": [{ id: "old" }] },
    read: async (key) => { if (key === "workspace-dirs" && failRead) throw new Error("retryable"); },
  });
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /retryable/);
  f.preferences.writeCanvasPreference("creative-library", [{ id: "local-edit" }]);
  await assert.rejects(f.preferences.whenCanvasPreferencesReady(), /retryable/);
  assert.deepEqual(f.preferences.readCanvasPreference("creative-library", null), [{ id: "local-edit" }]);
  failRead = false;
  const retry = f.preferences.whenCanvasPreferencesReady();
  assert.equal(retry, f.preferences.whenCanvasPreferencesReady());
  await retry;
  await f.preferences.flushCanvasPreferences();
  assert.deepEqual(f.persisted["creative-library"], [{ id: "local-edit" }]);
  assert.equal(f.reads.filter((key) => key === "creative-library").length, 1);
  assert.equal(f.reads.filter((key) => key === "workspace-dirs").length, 3);
  assert.equal(f.reads.filter((key) => key === "video-edit-projects").length, 1, "one failure must not skip other keys");
});

test("successful empty reads authorize creation and healthy directory edits retain the other field", async (t) => {
  const f = fixture(t);
  await f.preferences.whenCanvasPreferencesReady();
  for (const key of f.preferences.CANVAS_PREFERENCE_KEYS) {
    assert.equal(f.preferences.hasCanvasPreference(key), true);
    assert.equal(f.preferences.readCanvasPreference(key, "fallback"), null);
  }
  f.settings.getState().setWorkspaceDir("videoDir", "new-videos");
  f.settings.getState().setWorkspaceDir("imageDir", "new-images");
  const saving = f.preferences.writeCanvasPreferenceAndWait("video-edit-projects", { first: { id: "first" } });
  await f.preferences.flushCanvasPreferences();
  await saving;
  assert.deepEqual(f.persisted["workspace-dirs"], { imageDir: "new-images", videoDir: "new-videos" });
  assert.deepEqual(f.persisted["video-edit-projects"], { first: { id: "first" } });
  assert.equal(f.writes.length, 2);
});

test("browser-only preview remains ready without backend preference reads", async (t) => {
  const f = fixture(t, { tauri: false });
  await f.preferences.whenCanvasPreferencesReady();
  await f.preferences.flushCanvasPreferences();
  assert.deepEqual(f.reads, []);
  assert.deepEqual(f.writes, []);
  assert.equal(f.preferences.readCanvasPreference("workspace-dirs", "fallback"), "fallback");
});

test("the production lifecycle gate reports preference failure, retries and prevents unsafe close", async (t) => {
  let failRead = true;
  const f = fixture(t, { read: async (key) => {
    if (failRead && key === "workspace-dirs") throw new Error("blocked baseline");
  } });
  const states = [], effects = [], messages = [];
  let handler, closed = 0;
  const hook = loadTypeScript("../src/canvas/workflow/hooks/useWorkflowPersistence.ts", {
    react: {
      useEffect: (effect) => effects.push(effect),
      useState: (initial) => {
        const index = states.length;
        states.push(typeof initial === "function" ? initial() : initial);
        return [states[index], (next) => { states[index] = typeof next === "function" ? next(states[index]) : next; }];
      },
    },
    "@tauri-apps/api/core": { isTauri: () => true },
    "@tauri-apps/api/window": { getCurrentWindow: () => ({
      onCloseRequested: async (callback) => { handler = callback; return () => { handler = undefined; }; },
      close: async () => { closed++; },
    }) },
    "../utils/workflowPersistence": {
      workflowPersistenceProblems: () => [],
      subscribeWorkflowPersistence: () => () => {},
      whenWorkflowPersistenceReady: async () => {},
      flushWorkflowPersistence: async () => {},
    },
    "../utils/canvasPreferences": f.preferences,
    "../../../components/message": { message: { error: (text) => messages.push(text) } },
  }).useWorkflowPersistence();
  const cleanups = effects.map((effect) => effect());
  await tick();
  assert.equal(states[0], false, "graph ready must not authorize an editor with unread preferences");
  assert.match(states[1], /workspace-dirs.*blocked baseline/);
  let prevented = 0;
  await handler({ preventDefault: () => prevented++ });
  assert.equal(closed, 0);
  assert.equal(messages.length, 1);
  assert.equal(f.writes.length, 0);

  failRead = false;
  hook.retry();
  assert.equal(states[2], 1);
  cleanups[1]();
  cleanups[1] = effects[1]();
  await tick();
  assert.equal(states[0], true);
  assert.equal(states[1], null);
  f.settings.getState().setWorkspaceDir("imageDir", "recovered-images");
  await handler({ preventDefault: () => prevented++ });
  assert.equal(closed, 1);
  assert.equal(prevented, 2);
  assert.equal(f.persisted["workspace-dirs"].imageDir, "recovered-images");
  for (const cleanup of cleanups) cleanup?.();
  await tick();
  assert.equal(handler, undefined);
});
