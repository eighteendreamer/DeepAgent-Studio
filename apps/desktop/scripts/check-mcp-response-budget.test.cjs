// Execute the actual MCP edit serializer so hidden optional budgets survive edits.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

test("H126 actual MCP draftPayload preserves a configured budget without inventing one", () => {
  const file = path.resolve(__dirname, "../src/components/settings/MCPSettings.tsx");
  const source = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  let expression;
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "draftPayload") expression = node.getText(source);
    ts.forEachChild(node, visit);
  }
  visit(source); assert.ok(expression, "actual production serializer must exist");
  const compiled = ts.transpileModule(`module.exports = (${expression});`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  for (const budget of [undefined, 4096, 67108864]) {
    const draft = { name: "synthetic", transport: "stdio", command: "node", args: ["fixture.cjs"],
      env: {}, headers: {}, response_limit_bytes: budget };
    const loaded = { exports: {} };
    new Function("module", "draft", "envPairs", compiled)(loaded, draft, []);
    const saved = loaded.exports();
    assert.equal(saved.response_limit_bytes, budget);
    assert.equal(saved.source, "user"); assert.deepEqual(saved.args, draft.args);
    assert.equal(JSON.parse(JSON.stringify(saved)).response_limit_bytes, budget);
  }
});
