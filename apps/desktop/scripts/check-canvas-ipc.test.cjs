// Gate: the canvas talks to the kernel only through Tauri commands.
//
// Two failure modes this catches, both of which a build would otherwise ship
// silently: the UI invoking a command that was never registered (runtime
// "command not found" in the desktop app only), and the plan's required
// command surface losing an entry during a refactor.
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync, readdirSync, statSync } = require("node:fs");
const path = require("node:path");

const TAURI_LIB = path.resolve(__dirname, "../src-tauri/src/lib.rs");
const CANVAS_SRC = path.resolve(__dirname, "../src/canvas");

/** §15 "必要命令" of the canvas upgrade plan. */
const REQUIRED_COMMANDS = [
  "canvas_settings_read",
  "canvas_provider_save",
  "canvas_provider_remove",
  "canvas_model_save",
  "canvas_model_remove",
  "canvas_bindings_save",
  "canvas_secret_status",
  "canvas_secret_set",
  "canvas_secret_clear",
  "canvas_prompt_profiles_read",
  "canvas_prompt_resolve",
  "canvas_workflow_list",
  "canvas_workflow_load",
  "canvas_workflow_save",
  "canvas_workflow_delete",
  "canvas_artifact_import",
  "canvas_artifact_url",
];

function registeredCommands() {
  const source = readFileSync(TAURI_LIB, "utf8");
  const open = source.indexOf("generate_handler![");
  assert.ok(open >= 0, "lib.rs must register commands through generate_handler!");
  let depth = 0;
  let end = -1;
  for (let i = open + "generate_handler".length; i < source.length; i += 1) {
    const char = source[i];
    if (char === "[") depth += 1;
    else if (char === "]") {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  assert.ok(end > open, "generate_handler! list must be closed");
  return new Set(
    source
      .slice(open, end)
      .split("\n")
      .map((line) => line.trim().replace(/,$/, ""))
      .filter((line) => /^[a-z_][a-z0-9_]*$/.test(line)),
  );
}

function walk(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) return walk(full);
    return /\.(ts|tsx)$/.test(entry) ? [full] : [];
  });
}

function canvasCommandsUsedByUi() {
  const used = new Set();
  for (const file of walk(CANVAS_SRC)) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/invoke(?:<[^>]*>)?\(\s*"([a-z0-9_]+)"/g)) {
      used.add(match[1]);
    }
  }
  return used;
}

const registered = registeredCommands();

test("every command the canvas invokes is registered with tauri", () => {
  const missing = [...canvasCommandsUsedByUi()]
    .filter((name) => !registered.has(name))
    .sort();
  assert.deepEqual(
    missing,
    [],
    "the canvas calls commands the desktop build never registers",
  );
});

test("the plan's required canvas command surface stays registered", () => {
  const missing = REQUIRED_COMMANDS.filter((name) => !registered.has(name));
  assert.deepEqual(missing, [], `§15 commands gone: ${missing.join(", ")}`);
});
