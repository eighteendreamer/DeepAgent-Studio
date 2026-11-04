const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const { test } = require("node:test");
const ts = require("typescript");

function fixture(t, response, tauri = true) {
  const previousWindow = globalThis.window;
  globalThis.window = tauri ? { __TAURI_INTERNALS__: {} } : {};
  t.after(() => {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  });
  const calls = [];
  const filename = path.resolve(__dirname, "../src/api.ts");
  const compiled = ts.transpileModule(readFileSync(filename, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  });
  const loaded = new Module(filename, module);
  loaded.filename = filename;
  loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  loaded.require = (specifier) => {
    assert.equal(specifier, "@tauri-apps/api/core");
    return { invoke: async (command, args) => {
      calls.push({ command, args });
      if (response instanceof Error) throw response;
      return structuredClone(response);
    } };
  };
  loaded._compile(compiled.outputText, filename);
  return { search: loaded.exports.searchSessions, calls };
}

test("partial session search preserves good hits and explicit unavailable identities", async (t) => {
  const report = { version: 1, complete: false, unavailable: [{ session_id: "bad", kind: "corrupt" }], results: [{ session: { id: "good" }, snippet: "needle" }] };
  const f = fixture(t, report);
  assert.deepEqual(await f.search("needle", "A", 10), report);
  assert.deepEqual(f.calls, [{ command: "search_sessions", args: { query: "needle", project: "A", limit: 10 } }]);
});

test("healthy empty session search is still complete, unlike backend failure", async (t) => {
  const report = { version: 1, complete: true, unavailable: [], results: [] };
  const f = fixture(t, report);
  assert.deepEqual(await f.search("missing"), report);
});

test("deferred maintenance is explicit even with no matching results", async (t) => {
  const report = { version: 1, complete: false, unavailable: [{ session_id: "pending", kind: "deferred" }], results: [] };
  const f = fixture(t, report);
  assert.deepEqual(await f.search("needle"), report);
});

test("unversioned, unsupported and contradictory session search envelopes are errors", async (t) => {
  const reports = [[], null, { version: 2, complete: true, unavailable: [], results: [] },
    { version: 1, complete: true, unavailable: [{ session_id: "bad", kind: "corrupt" }], results: [] },
    { version: 1, complete: false, unavailable: [], results: [] }];
  for (const [index, report] of reports.entries()) {
    await t.test(`invalid envelope ${index}`, async (child) => {
      const f = fixture(child, report);
      await assert.rejects(f.search("needle"), /session search.*report/i);
    });
  }
});

test("search bridge preserves global database errors instead of returning empty results", async (t) => {
  const f = fixture(t, new Error("synthetic SQLite projection failure"));
  await assert.rejects(f.search("needle"), /SQLite projection failure/);
});

test("plain browser cannot claim the persisted session search completed", async (t) => {
  const f = fixture(t, undefined, false);
  await assert.rejects(f.search("needle"), /desktop/i);
  assert.deepEqual(f.calls, []);
});
