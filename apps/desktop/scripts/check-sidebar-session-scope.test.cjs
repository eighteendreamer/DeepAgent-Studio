const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const ts = require("typescript");

const source = readFileSync(join(__dirname, "..", "src", "components", "sidebarSessions.ts"), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;
const moduleExports = {};
vm.runInNewContext(compiled, { exports: moduleExports });
const { partitionSidebarSessions } = moduleExports;

const session = (id, project, pinned, created_at, updated_at) => ({
  id, project, pinned, created_at, updated_at,
});
const ids = (items) => Array.from(items, (item) => item.id);

test("project conversations never appear in recent conversations", () => {
  const sessions = [
    session("project", "DeepAgent", false, 3, 30),
    session("unassigned", null, false, 2, 20),
    session("pinned-project", "DeepAgent", true, 1, 10),
    session("pinned-unassigned", undefined, true, 4, 40),
  ];
  const result = partitionSidebarSessions(sessions, "updated");
  assert.deepEqual(ids(result.projectSessions), ["pinned-project", "project"]);
  assert.deepEqual(ids(result.recentSessions), ["pinned-unassigned", "unassigned"]);
  assert.deepEqual(ids(sessions), ["project", "unassigned", "pinned-project", "pinned-unassigned"]);
});

test("created and updated sorting stay within each owner", () => {
  const sessions = [
    session("old-project", "A", false, 1, 9),
    session("new-project", "A", false, 3, 2),
    session("old-recent", null, false, 2, 8),
    session("new-recent", null, false, 4, 1),
  ];
  const byCreation = partitionSidebarSessions(sessions, "created");
  assert.deepEqual(ids(byCreation.projectSessions), ["new-project", "old-project"]);
  assert.deepEqual(ids(byCreation.recentSessions), ["new-recent", "old-recent"]);
  const byUpdate = partitionSidebarSessions(sessions, "updated");
  assert.deepEqual(ids(byUpdate.projectSessions), ["old-project", "new-project"]);
  assert.deepEqual(ids(byUpdate.recentSessions), ["old-recent", "new-recent"]);
});
