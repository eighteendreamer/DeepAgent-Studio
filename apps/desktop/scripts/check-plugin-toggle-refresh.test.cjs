const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const source = readFileSync(
  join(__dirname, "..", "src", "components", "PluginsViewReal.tsx"),
  "utf8",
);

test("plugin toggle refreshes plugin state without a full page reload", () => {
  const start = source.indexOf("const applyPluginToggle = async");
  const end = source.indexOf("const togglePlugin = async", start);

  assert.notEqual(start, -1, "applyPluginToggle must exist");
  assert.notEqual(end, -1, "togglePlugin must follow applyPluginToggle");

  const implementation = source.slice(start, end);
  assert.doesNotMatch(implementation, /\bload\(\)/);
  assert.match(implementation, /setPlugins\(/);
  assert.match(implementation, /listPlugins\(\)/);
  assert.match(implementation, /if \(selectedId === plugin\.id\)/);
  assert.match(implementation, /listPluginOutputStyles\(\)/);
  assert.doesNotMatch(
    implementation,
    /listPluginMarketplaces|listPluginMarketplaceEntries/,
  );
});

test("plugin catalog loads details only after selection", () => {
  const loadStart = source.indexOf("const load = async");
  const loadEnd = source.indexOf("useEffect(() => {", loadStart);
  assert.notEqual(loadStart, -1);
  assert.notEqual(loadEnd, -1);
  const catalogLoad = source.slice(loadStart, loadEnd);
  assert.match(catalogLoad, /listPlugins\(\)/);
  assert.doesNotMatch(catalogLoad, /readPlugin\(|listPluginOutputStyles\(/);

  assert.match(source, /if \(!selectedId\)/);
  assert.match(source, /void readPlugin\(selectedId\)/);
  assert.match(source, /void listPluginOutputStyles\(\)/);
  const rowStart = source.indexOf("function PluginRow(");
  const rowEnd = source.indexOf("function PluginDetail(", rowStart);
  assert.doesNotMatch(source.slice(rowStart, rowEnd), /PluginStateBadges/);
});

test("tool launchers use enabled plugin apps instead of static plugin cards", () => {
  const hook = readFileSync(
    join(__dirname, "..", "src", "components", "plugins", "usePluginAppCards.ts"),
    "utf8",
  );
  assert.match(hook, /listPluginApps\(\)/);
  assert.match(hook, /PLUGINS_CHANGED_EVENT/);

  for (const name of ["RightSidebarWorkbench.tsx", "ChatView.tsx", "StartView.tsx"]) {
    const launcher = readFileSync(join(__dirname, "..", "src", "components", name), "utf8");
    assert.match(launcher, /usePluginAppCards\(/, `${name} must use enabled plugin apps`);
    assert.doesNotMatch(launcher, /PLUGIN_TOOL_CARDS/, `${name} must not expose static plugin cards`);
  }
});

test("right sidebar tools render component icons for every built-in tool type", () => {
  const components = join(__dirname, "..", "src", "components");
  const icons = readFileSync(join(components, "plugins", "toolIconComponents.ts"), "utf8");
  const launcher = readFileSync(join(components, "ToolLauncherPanel.tsx"), "utf8");
  const header = readFileSync(join(components, "SidebarPluginHeader.tsx"), "utf8");

  for (const type of ["browser", "canvas", "chat", "file_preview", "files", "project_map", "recording", "terminal"]) {
    assert.match(icons, new RegExp(`\\b${type}:\\s*[A-Z]`), `${type} needs a component icon`);
  }
  assert.match(launcher, /const Icon = toolIconComponent\(card\.type\)/);
  assert.match(launcher, /<Icon className="h-\[18px\] w-\[18px\]"/);
  assert.match(header, /toolIconComponent\(tab\.type\)/);
  assert.match(header, /toolIconComponent\(plugin\.type\)/);
  assert.match(header, /items-center justify-center text-text-secondary">\s*<Icon/);
  assert.doesNotMatch(header, /bg-primary\/10 text-primary/);
});

test("tool panels use component icons without Font Awesome renderers", () => {
  const components = join(__dirname, "..", "src", "components");
  for (const path of [
    ["plugins", "BrowserPlugin.tsx"],
    ["plugins", "FilesPlugin.tsx"],
    ["plugins", "RecordingPlugin.tsx"],
    ["project-map", "ProjectMapPanel.tsx"],
  ]) {
    const panel = readFileSync(join(components, ...path), "utf8");
    assert.match(panel, /from "lucide-react"/, `${path.join("/")} needs component icons`);
    assert.doesNotMatch(panel, /FontAwesomeIcon/, `${path.join("/")} must not render legacy icons`);
  }
});

test("built-in sidebar tab titles use the active locale without replacing custom titles", () => {
  const header = readFileSync(join(__dirname, "..", "src", "components", "SidebarPluginHeader.tsx"), "utf8");
  assert.match(header, /explicitLabel\(tab\.title, tab\.type\)/);
  assert.match(header, /t\(`chatView\.tools\.\$\{tab\.type\}`/);
  assert.match(header, /normalized === "meeting_recorder"/);
});

test("project map keeps a scrollable readable canvas in a narrow sidebar", () => {
  const panel = readFileSync(join(__dirname, "..", "src", "components", "project-map", "ProjectMapPanel.tsx"), "utf8");
  assert.match(panel, /ref=\{viewportRef\} className="absolute inset-0 overflow-auto"/);
  assert.match(panel, /min-h-\[640px\].*min-w-\[1000px\]/);
  assert.match(panel, /viewport\.scrollLeft = Math\.max/);
  assert.match(panel, /statusLabel\(status\)/);
});
