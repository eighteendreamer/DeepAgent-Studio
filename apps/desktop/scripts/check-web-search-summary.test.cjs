const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const test = require("node:test");
const ts = require("typescript");

const filename = path.resolve(__dirname, "../src/components/chat-timeline/webSearchSummary.ts");
const compiled = ts.transpileModule(fs.readFileSync(filename, "utf8"), {
  fileName: filename, compilerOptions: { module: ts.ModuleKind.CommonJS },
});
const loaded = new Module(filename, module);
loaded._compile(compiled.outputText, filename);
const { summarizeWebSearch } = loaded.exports;

test("live search summary reads unified evidence and reports incomplete coverage", () => {
  const output = { count: 2, results: [{}, {}], evidence: { incomplete: true, attempts: [
    { engine: "deepseek", ok: false, result_count: 0 },
    { engine: "anysearch", ok: true, result_count: 2 },
    { engine: "searxng", ok: true, result_count: 0 },
  ] } };
  assert.equal(summarizeWebSearch(output), "web_search: anysearch returned 2 result(s); attempts: deepseek:failed, anysearch:ok, searxng:ok; coverage incomplete");
});
test("historical search metadata remains readable without rewriting stored output", () => {
  const output = { provider: "duckduckgo", count: 1, attempts: [{ provider: "duckduckgo", ok: true }] };
  assert.equal(summarizeWebSearch(output), "web_search: duckduckgo returned 1 result(s); attempts: duckduckgo:ok");
});
