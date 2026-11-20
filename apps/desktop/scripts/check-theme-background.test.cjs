const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");

const image = { dataUrl: "data:image/webp;base64,UklGRg==", name: "wallpaper.webp" };
const plain = value => JSON.parse(JSON.stringify(value));

test("the main native window permits the sidebar material to show through the webview", () => {
  const config = JSON.parse(fs.readFileSync(
    path.resolve(__dirname, "../src-tauri/tauri.conf.json"), "utf8",
  ));
  const main = config.app.windows.find(window => window.label === "main");
  assert.equal(main.transparent, true,
    "Mica requires a transparent native window, even when CSS backgrounds are transparent");
});

test("wallpaper translations coexist with the theme background color label", () => {
  for (const language of ["zh", "en"]) {
    const appearance = JSON.parse(fs.readFileSync(
      path.resolve(__dirname, `../src/locales/${language}.json`), "utf8",
    )).settings.appearance;
    assert.equal(typeof appearance.background, "string");
    for (const key of ["title", "description", "empty", "upload", "replace", "remove", "transparency"]) {
      assert.equal(typeof appearance.wallpaper[key], "string", `${language}: ${key}`);
    }
  }
});

function host(options = {}) {
  const values = new Map(), properties = new Map(), classes = new Set();
  const revoked = [], dimensions = [], states = [], posted = [], effects = [], listeners = {};
  let urls = 0, encodes = 0, quotaFailure = false;
  const root = {
    style: { setProperty: (key, value) => properties.set(key, value), removeProperty: key => properties.delete(key) },
    classList: {
      add: key => classes.add(key), remove: key => classes.delete(key),
      toggle(key, enabled) { if (enabled) classes.add(key); else classes.delete(key); },
    },
  };
  const canvas = { width: 0, height: 0,
    getContext: () => ({ drawImage() { dimensions.push([canvas.width, canvas.height]); } }),
    toDataURL() { return ++encodes < (options.fitsOnAttempt ?? 1) ? image.dataUrl + "A".repeat(1_048_576) : image.dataUrl; },
  };
  const globals = {
    console, TextEncoder, TextDecoder, btoa, atob, URLSearchParams,
    localStorage: {
      getItem: key => values.get(key) ?? null,
      setItem(key, value) { if (quotaFailure) throw new Error("QuotaExceededError"); values.set(key, value); },
    },
    document: { documentElement: root, createElement: tag => {
      assert.equal(tag, "canvas"); return canvas;
    } },
    window: {
      location: { hash: options.hash ?? "" },
      matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
      addEventListener: (name, handler) => { listeners[name] = handler; }, removeEventListener() {},
      setTimeout: () => 1, clearTimeout() {},
    },
    BroadcastChannel: class {
      constructor() { channels.push(this); }
      postMessage(value) { posted.push(value); }
      close() {}
    },
    URL: { createObjectURL() { urls++; return "blob:fixture"; }, revokeObjectURL(url) { revoked.push(url); } },
    Image: class {
      naturalWidth = options.width ?? 3200;
      naturalHeight = options.height ?? 1800;
      set src(_value) { if (options.decodeFailure) this.onerror(); else this.onload(); }
    },
  };
  const channels = [], cache = new Map();
  const react = {
    createContext: () => ({ Provider: "Provider" }),
    useState(value) { return [typeof value === "function" ? value() : value, value => states.push(value)]; },
    useRef: value => ({ current: value }), useMemo: fn => fn(), useCallback: fn => fn,
    useEffect: fn => effects.push(fn),
  };
  function load(relative) {
    const filename = path.resolve(__dirname, "../src/theme", relative);
    if (cache.has(filename)) return cache.get(filename).exports;
    const module = { exports: {} }; cache.set(filename, module);
    const source = ts.transpileModule(fs.readFileSync(filename, "utf8"), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
    }).outputText;
    vm.runInNewContext(source, { ...globals, module, exports: module.exports, require(name) {
      if (name === "react") return react;
      if (name === "react/jsx-runtime") return { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }) };
      if (name.endsWith(".css")) return {};
      if (name === "../api") return { setWindowTranslucent: async () => {} };
      assert.ok(name.startsWith("./"), `Unexpected dependency: ${name}`);
      return load(name.replace("./", "") + ".ts");
    } }, { filename });
    return module.exports;
  }
  return { load, values, properties, classes, dimensions, revoked, states, posted, channels, listeners,
    flushEffects: () => effects.splice(0).map(fn => fn()),
    get urls() { return urls; }, set quotaFailure(value) { quotaFailure = value; },
  };
}

test("old theme preferences gain background defaults without losing palettes or changing the input", () => {
  const h = host(), storage = h.load("themeStorage.ts");
  const old = plain(storage.defaultV2State());
  delete old.preferences.backgroundImage; delete old.preferences.backgroundTransparency;
  old.preferences.uiFont = "Existing Font";
  const next = storage.migrateThemeState(old);
  assert.equal(next.preferences.backgroundImage, null);
  assert.equal(next.preferences.backgroundTransparency, 75);
  assert.equal(next.preferences.uiFont, "Existing Font");
  assert.deepEqual(plain(next.workingPalettes), old.workingPalettes);
  assert.equal("backgroundImage" in old.preferences, false);
});

test("Codex remains the default in both modes with its existing palette intact", () => {
  const h = host(), presets = h.load("themePresets.ts");
  const state = h.load("themeStorage.ts").defaultV2State();
  assert.equal(presets.BUILTIN_PRESETS[0].id, "codex");
  assert.deepEqual(plain(state.selectedPresetIds), { light: "codex", dark: "codex" });
  assert.deepEqual(plain(state.workingPalettes), {
    light: { accent: "#339CFF", accentHover: "#2563EB", background: "#FFFFFF",
      sidebar: "#F9F8F6", elevated: "#FFFFFF", foreground: "#1F2937",
      foregroundMuted: "#6B7280", border: "#E5E7EB", hover: "#F3F4F6",
      selection: "#DBEAFE", contrast: 45 },
    dark: { accent: "#339CFF", accentHover: "#60A5FA", background: "#000000",
      sidebar: "#121212", elevated: "#1A1A1A", foreground: "#F9FAFB",
      foregroundMuted: "#9CA3AF", border: "#27272A", hover: "#1F1F23",
      selection: "#1E3A8A", contrast: 50 },
  });
});

test("additional palettes have distinct surfaces and readable text after token derivation", () => {
  const h = host(), presets = h.load("themePresets.ts");
  const { paletteToTokens } = h.load("themeTokens.ts");
  const { contrastRatio } = h.load("colorUtils.ts");
  const ids = ["graphite", "paper", "mist-blue", "pine", "dusk-violet", "amber"];
  const lowContrast = [];
  for (const variant of ["light", "dark"]) {
    const backgrounds = new Set();
    for (const id of ids) {
      const preset = presets.getPresetById(id);
      assert.ok(preset?.variants[variant], `${id}/${variant}`);
      const tokens = paletteToTokens(preset.variants[variant]);
      backgrounds.add(tokens["--theme-bg"]);
      assert.equal(new Set([tokens["--theme-bg"], tokens["--theme-sidebar"], tokens["--theme-elevated"]]).size, 3);
      for (const surface of ["--theme-bg", "--theme-sidebar", "--theme-elevated", "--theme-selection"]) {
        for (const foreground of ["--theme-fg", "--theme-text-secondary"]) {
          const ratio = contrastRatio(tokens[foreground], tokens[surface]);
          if (ratio < 4.5) lowContrast.push(`${id}/${variant}: ${foreground} on ${surface} (${ratio.toFixed(2)})`);
        }
      }
      assert.ok(contrastRatio(tokens["--theme-accent"], tokens["--theme-selection"]) >= 4.5,
        `${id}/${variant}: selected text`);
    }
    assert.equal(backgrounds.size, ids.length);
  }
  assert.deepEqual(lowContrast, []);
  for (const id of ["absolutely", "ayu", "catppuccin", "dracula", "everforest", "github", "gruvbox", "linear", "notion", "one"]) {
    assert.ok(presets.getPresetById(id), `${id} is still available`);
  }
});

test("selecting an additional preset persists per mode and returns to Codex without migration", () => {
  const h = host(), storage = h.load("themeStorage.ts"), presets = h.load("themePresets.ts");
  const initial = storage.defaultV2State();
  initial.customPresets = [{ id: "custom-existing", name: "Existing", source: "custom",
    variants: { light: { ...initial.workingPalettes.light, accent: "#123456" } } }];
  storage.saveThemeState(initial);
  const context = h.load("ThemeProvider.tsx").ThemeProvider({ children: null }).props.value;
  context.selectPreset("light", "paper");
  context.selectPreset("dark", "mist-blue");
  let loaded = storage.loadThemeState();
  assert.deepEqual(plain(loaded.selectedPresetIds), { light: "paper", dark: "mist-blue" });
  assert.deepEqual(plain(loaded.workingPalettes.light), plain(presets.getPresetById("paper").variants.light));
  assert.deepEqual(plain(loaded.workingPalettes.dark), plain(presets.getPresetById("mist-blue").variants.dark));
  assert.deepEqual(plain(loaded.preferences), plain(initial.preferences));
  assert.deepEqual(plain(loaded.customPresets), plain(initial.customPresets));
  context.selectPreset("light", "codex");
  loaded = storage.loadThemeState();
  assert.deepEqual(plain(loaded.workingPalettes.light), plain(initial.workingPalettes.light));
  assert.equal(loaded.selectedPresetIds.dark, "mist-blue");
});

test("persisted images admit bounded raster data only and transparency has a finite range", () => {
  const background = host().load("themeBackground.ts");
  for (const value of [null, {}, { dataUrl: "https://example.com/image.png" },
    { dataUrl: 'data:image/svg+xml;base64,PHN2Zz4=' },
    { dataUrl: image.dataUrl + 'A'.repeat(background.MAX_BACKGROUND_DATA_URL_LENGTH) }]) {
    assert.equal(background.normalizeBackgroundImage(value), null);
  }
  assert.deepEqual(plain(background.normalizeBackgroundImage(image)), image);
  assert.equal(background.normalizeBackgroundTransparency(-12), 0);
  assert.equal(background.normalizeBackgroundTransparency(150), 100);
  assert.equal(background.normalizeBackgroundTransparency(NaN), 75);
});

test("transparency controls surface opacity in the expected direction and removing wallpaper restores sidebar translucency", () => {
  const h = host(), background = h.load("themeBackground.ts");
  const preferences = { ...h.load("themeStorage.ts").defaultV2State().preferences, backgroundImage: image };
  assert.equal(background.applyThemeBackground(preferences), false);
  assert.ok(h.classes.has("theme-custom-background"));
  assert.equal(h.properties.get("--app-surface-opacity"), "0.25");
  assert.equal(h.properties.has("--app-background-opacity"), false);
  assert.equal(h.properties.has("opacity"), false);
  for (const [transparency, opacity] of [[0, "1"], [71, "0.29"], [100, "0"]]) {
    background.previewBackgroundTransparency(transparency);
    assert.equal(h.properties.get("--app-surface-opacity"), opacity);
  }
  assert.equal(background.applyThemeBackground({ ...preferences, backgroundImage: null }), true);
  assert.ok(!h.classes.has("theme-custom-background"));
  assert.ok(h.classes.has("theme-translucent-sidebar"));
  assert.equal(h.properties.has("--app-background-image"), false);
});

test("the canvas window ignores wallpaper without changing the shared saved preference", () => {
  const h = host({ hash: "#window=canvas" });
  const preferences = { ...h.load("themeStorage.ts").defaultV2State().preferences, backgroundImage: image };
  assert.equal(h.load("themeBackground.ts").applyThemeBackground(preferences), false);
  assert.ok(!h.classes.has("theme-custom-background"));
  assert.ok(!h.classes.has("theme-translucent-sidebar"));
  assert.equal(h.properties.has("--app-background-image"), false);
  assert.deepEqual(plain(preferences.backgroundImage), image);
});

test("unsupported and oversized files are rejected before decoding or allocating an object URL", async () => {
  const h = host(), background = h.load("themeBackground.ts");
  await assert.rejects(background.prepareBackgroundImage({ type: "image/svg+xml", size: 20 }), error => error.code === "unsupported");
  await assert.rejects(background.prepareBackgroundImage({ type: "image/png", size: 11 * 1024 * 1024 }), error => error.code === "tooLarge");
  assert.equal(h.urls, 0);
});

test("decoding failure releases the image URL and never produces an accepted background", async () => {
  const h = host({ decodeFailure: true });
  await assert.rejects(h.load("themeBackground.ts").prepareBackgroundImage({ type: "image/png", size: 100 }), error => error.code === "decode");
  assert.deepEqual(h.revoked, ["blob:fixture"]);
  assert.deepEqual(h.dimensions, []);
});

test("large images are resized until the stored data fits and decoded URLs are always released", async () => {
  const h = host({ fitsOnAttempt: 2 });
  const result = await h.load("themeBackground.ts").prepareBackgroundImage({ type: "image/png", size: 200, name: "large.png" });
  assert.deepEqual(plain(result), { ...image, name: "large.png" });
  assert.deepEqual(h.dimensions, [[2560, 1440], [1920, 1080]]);
  assert.deepEqual(h.revoked, ["blob:fixture"]);
});

test("reload and startup restore the same wallpaper, transparency and theme variant", () => {
  const h = host(), storage = h.load("themeStorage.ts");
  const state = storage.defaultV2State(); state.mode = "dark";
  state.preferences.backgroundImage = image; state.preferences.backgroundTransparency = 40;
  storage.saveThemeState(state);
  assert.deepEqual(plain(storage.loadThemeState()), plain(state));
  h.load("themeBootstrap.ts").bootstrapTheme();
  assert.ok(h.classes.has("dark")); assert.ok(h.classes.has("theme-custom-background"));
  assert.equal(h.properties.get("--app-surface-opacity"), "0.6");
  assert.ok(h.properties.get("--app-background-image").includes(image.dataUrl));
});

test("failed persistence changes neither provider state nor the saved background or revision", () => {
  const h = host();
  const context = h.load("ThemeProvider.tsx").ThemeProvider({ children: null }).props.value;
  context.updatePreferences({ backgroundImage: image });
  const saved = h.values.get("deepagent:theme:v2"), count = h.states.length;
  h.quotaFailure = true;
  assert.throws(() => context.updatePreferences({ backgroundImage: null }), /QuotaExceededError/);
  assert.equal(h.states.length, count); assert.equal(h.values.get("deepagent:theme:v2"), saved);
  h.quotaFailure = false;
  context.updatePreferences({ backgroundTransparency: 15 });
  const next = JSON.parse(h.values.get("deepagent:theme:v2"));
  assert.deepEqual(next.preferences.backgroundImage, image);
  assert.equal(next.revision, 2);
});

test("multi-window broadcasts and storage updates normalize incoming background preferences", () => {
  const h = host();
  const context = h.load("ThemeProvider.tsx").ThemeProvider({ children: null }).props.value;
  h.flushEffects();
  context.updatePreferences({ backgroundImage: image, backgroundTransparency: 30 });
  assert.deepEqual(plain(h.posted[0].preferences.backgroundImage), image);
  const incoming = plain(h.posted[0]); incoming.revision++;
  incoming.preferences.backgroundImage = { dataUrl: "https://example.com/tracker.png" };
  h.channels[0].onmessage({ data: incoming });
  assert.equal(h.states.at(-1).preferences.backgroundImage, null);
  incoming.revision++; incoming.preferences.backgroundImage = image;
  h.listeners.storage({ key: "deepagent:theme:v2", newValue: JSON.stringify(incoming) });
  assert.deepEqual(plain(h.states.at(-1).preferences.backgroundImage), image);
});
