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

test("canvas preference hydration must not overwrite an edit made before read completes", {
  skip: process.env.RUN_HARNESS_AUDIT !== "1",
  timeout: 5000,
}, async () => {
  const previousWindow = globalThis.window;
  const previousLocalStorage = globalThis.localStorage;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  globalThis.localStorage = {
    getItem: () => null,
    removeItem: () => {},
  };

  const persisted = {
    "workspace-dirs": { imageDir: "old-images", videoDir: "old-videos" },
  };
  const writes = [];
  let releaseWorkspaceRead;
  const workspaceRead = new Promise((resolve) => {
    releaseWorkspaceRead = () => resolve(structuredClone(persisted["workspace-dirs"]));
  });
  const invoke = async (command, args) => {
    if (command === "canvas_preference_read") {
      if (args.key === "workspace-dirs") return workspaceRead;
      return persisted[args.key] ?? null;
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

  try {
    const preferences = loadTypeScript("../src/canvas/workflow/utils/canvasPreferences.ts", {
      "@tauri-apps/api/core": { invoke },
      "./canvasMedia": { importCanvasMedia: async () => "artifact://audit" },
    });
    const settings = loadTypeScript("../src/canvas/workflow/store/canvasSettingsStore.ts", {
      "@tauri-apps/api/core": { invoke },
      "../utils/canvasPreferences": preferences,
    });

    // The production store is available synchronously while its module-level
    // preference hydration is still waiting on the backend read.
    settings.useCanvasSettingsStore.getState().setWorkspaceDir("imageDir", "new-images");
    const beforeHydration = settings.useCanvasSettingsStore.getState().workspace;
    assert.equal(beforeHydration.imageDir, "new-images");

    releaseWorkspaceRead();
    await preferences.whenCanvasPreferencesReady();
    await new Promise((resolve) => setTimeout(resolve, 500));
    await preferences.flushCanvasPreferences();

    const afterHydration = settings.useCanvasSettingsStore.getState().workspace;
    const write = writes.find((entry) => entry.key === "workspace-dirs");
    console.log(JSON.stringify({
      editBeforeRead: beforeHydration,
      stateAfterHydration: afterHydration,
      writePayload: write?.value ?? null,
      persistedAfter: persisted["workspace-dirs"],
    }));
    assert.deepEqual(
      write?.value,
      { imageDir: "new-images", videoDir: "old-videos" },
      "a successful late preference read must not overwrite a user edit made before hydration",
    );
  } finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
    if (previousLocalStorage === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = previousLocalStorage;
  }
});
