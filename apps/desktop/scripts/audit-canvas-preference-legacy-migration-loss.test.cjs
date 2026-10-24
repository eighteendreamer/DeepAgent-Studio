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

test("legacy canvas preference must survive a failed migration write", {
  skip: process.env.RUN_HARNESS_AUDIT !== "1",
  timeout: 5000,
}, async () => {
  const previousWindow = globalThis.window;
  const previousLocalStorage = globalThis.localStorage;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const legacy = new Map([
    ["workflow-workspace", JSON.stringify({ imageDir: "legacy-images", videoDir: "legacy-videos" })],
  ]);
  globalThis.localStorage = {
    getItem: (key) => legacy.get(key) ?? null,
    removeItem: (key) => legacy.delete(key),
  };

  const invoke = async (command) => {
    if (command === "canvas_preference_read") return null;
    if (command === "canvas_preference_write") throw new Error("simulated migration write failure");
    throw new Error(`unexpected Tauri command: ${command}`);
  };

  try {
    const preferences = loadTypeScript("../src/canvas/workflow/utils/canvasPreferences.ts", {
      "@tauri-apps/api/core": { invoke },
      "./canvasMedia": { importCanvasMedia: async () => "artifact://audit" },
    });

    await preferences.whenCanvasPreferencesReady();
    console.log(JSON.stringify({
      legacyAfter: legacy.get("workflow-workspace") ?? null,
      cacheLoaded: preferences.hasCanvasPreference("workspace-dirs"),
    }));
    assert.equal(
      legacy.get("workflow-workspace"),
      JSON.stringify({ imageDir: "legacy-images", videoDir: "legacy-videos" }),
      "failed migration must retain the legacy source for retry",
    );
  } finally {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
    if (previousLocalStorage === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = previousLocalStorage;
  }
});
