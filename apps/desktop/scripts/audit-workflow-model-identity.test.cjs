const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
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
  loaded.require = (specifier) => Object.hasOwn(imports, specifier)
    ? imports[specifier]
    : requireModule(specifier);
  loaded._compile(compiled.outputText, filename);
  return loaded.exports;
}

const schemas = loadTypeScript("../src/canvas/workflow/utils/nodeSchemas.ts");
const configSchema = loadTypeScript("../src/canvas/workflow/utils/configSchema.ts");
const registry = loadTypeScript("../src/canvas/workflow/utils/nodeRegistry.ts", {
  "./nodeSchemas": schemas,
  "./configSchema": configSchema,
});

const modelFields = [
  ["llm", "llmModel"],
  ["question-classifier", "classifierModel"],
  ["parameter-extractor", "extractorModel"],
  ["agent-v2", "agentV2Model"],
];

function isAutomaticOrQualifiedModelRef(value) {
  return value === "" || /^[^:]+::.+$/.test(value);
}

test("new workflow nodes use automatic routing or a provider-qualified model reference", () => {
  const observed = modelFields.map(([kind, field]) => {
    const data = registry.createDefaultNodeData(kind);
    const schemaDefault = registry.getNodeDefinition(kind).configSchema.properties[field].default;
    return { kind, field, nodeDefault: data[field], schemaDefault };
  });
  process.stderr.write(`workflow model identity audit: ${JSON.stringify(observed)}\n`);

  for (const entry of observed) {
    assert.equal(
      isAutomaticOrQualifiedModelRef(entry.nodeDefault),
      true,
      `${entry.kind}.${entry.field} must be empty or providerId::modelId`,
    );
    assert.equal(
      isAutomaticOrQualifiedModelRef(entry.schemaDefault),
      true,
      `${entry.kind}.${entry.field} schema default must be empty or providerId::modelId`,
    );
  }
});
