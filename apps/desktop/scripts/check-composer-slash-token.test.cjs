const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

const file = path.join(__dirname, "../src/components/composerSlashToken.ts");
const compiled = ts.transpileModule(fs.readFileSync(file, "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS },
});
const api = {};
new Function("exports", compiled.outputText)(api);
const { findCommittedSlash, projectSlashDraft, restoreSlashDraft, slashCommandKind, toSlashEditorOffset } = api;
const titles = new Set(["/context", "/model", "/plugin:command"]);

test("slash chips require a registered leading command and a space", () => {
  for (const text of ["/", "/con", "/context", "/context/notes", "/unknown x", "请看 /context x"]) {
    assert.equal(findCommittedSlash(text, titles), null, text);
  }
  assert.deepEqual(findCommittedSlash("/context ", titles), { prefix: "/context ", label: "Context", kind: "command" });
  assert.equal(findCommittedSlash("/plugin:command 中文", titles)?.prefix, "/plugin:command ");
});

test("plugin aliases and namespaced commands are distinct from skill and ordinary commands", () => {
  assert.equal(slashCommandKind("/plugin"), "plugin");
  assert.equal(slashCommandKind("/plugins"), "plugin");
  assert.equal(findCommittedSlash("/plugin:command arg", titles)?.kind, "plugin");
  assert.equal(slashCommandKind("/skills"), "skill");
  assert.equal(slashCommandKind("/context"), "command");
});

test("an atomic editor selection maps to the full command and leaves arguments intact", () => {
  const draft = "/context 示例 🙂\n第二行";
  const { prefix } = findCommittedSlash(draft, titles);
  const editor = projectSlashDraft(draft, prefix);
  const decoded = restoreSlashDraft(editor, prefix);
  assert.equal(decoded.draft, draft);
  assert.equal(decoded.draft.slice(decoded.offset(0), decoded.offset(1)), "/context ");
  assert.equal(toSlashEditorOffset(prefix.length, prefix), 1);
  assert.equal(toSlashEditorOffset(draft.length, prefix), editor.length);
  assert.equal(restoreSlashDraft(editor.slice(1), prefix).draft, "示例 🙂\n第二行");
  assert.equal(restoreSlashDraft(`前缀 ${editor}`, prefix).draft, `前缀 ${draft}`);
});
