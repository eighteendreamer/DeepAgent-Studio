const assert = require("node:assert/strict");
const { existsSync, readFileSync, readdirSync } = require("node:fs");
const { dirname, join, resolve } = require("node:path");
const test = require("node:test");

const desktopRoot = resolve(__dirname, "..");
const pluginsRoot = join(desktopRoot, "src-tauri", "resources", "plugins");
const pluginsViewPath = join(desktopRoot, "src", "components", "PluginsViewReal.tsx");

test("every bundled plugin declares logo assets that exist", () => {
  const pluginNames = readdirSync(pluginsRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort();

  assert.equal(pluginNames.length, 12, "the bundled plugin inventory changed");
  for (const pluginName of pluginNames) {
    const manifestPath = join(pluginsRoot, pluginName, ".codex-plugin", "plugin.json");
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    const composerIcon = manifest.interface?.composerIcon;
    const logo = manifest.interface?.logo;

    assert.equal(typeof composerIcon, "string", `${pluginName} is missing composerIcon`);
    assert.equal(typeof logo, "string", `${pluginName} is missing logo`);
    assert.ok(
      existsSync(resolve(dirname(manifestPath), "..", composerIcon)),
      `${pluginName} composerIcon does not exist: ${composerIcon}`,
    );
    assert.ok(
      existsSync(resolve(dirname(manifestPath), "..", logo)),
      `${pluginName} logo does not exist: ${logo}`,
    );
  }
});

test("plugin cards render bundled image paths through the Tauri asset protocol", () => {
  const source = readFileSync(pluginsViewPath, "utf8");

  assert.match(source, /import \{ convertFileSrc \} from "@tauri-apps\/api\/core";/);
  assert.match(source, /plugin\.icon_path \|\| plugin\.logo_path/);
  assert.match(source, /convertFileSrc\(assetPath\)/);
  assert.match(source, /onError=/);
});
