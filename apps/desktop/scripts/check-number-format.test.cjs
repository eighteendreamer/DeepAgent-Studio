const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

const sourceRoot = path.resolve(__dirname, "../src");
const formatPath = path.join(sourceRoot, "components/chat-timeline/format.ts");
const CNY = "￥"; // full-width sign, same as format.ts

function sourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(entryPath);
    return /\.(ts|tsx)$/.test(entry.name) ? [entryPath] : [];
  });
}

// format.ts is dependency-free, so the shipped implementation can be executed
// directly instead of asserting on its source text.
function loadFormatters() {
  const { outputText } = ts.transpileModule(fs.readFileSync(formatPath, "utf8"), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  });
  const module = { exports: {} };
  new Function("exports", "module", outputText)(module.exports, module);
  return module.exports;
}

const { formatTokens, formatCny } = loadFormatters();

test("token counts keep one decimal at every magnitude", () => {
  assert.equal(formatTokens(999), "999");
  assert.equal(formatTokens(1000), "1k");
  // The per-turn footer used to print `16k` here while the totals bar printed
  // `15.8k` for the same number.
  assert.equal(formatTokens(15843), "15.8k");
  assert.equal(formatTokens(16000), "16k");
  assert.equal(formatTokens(999499), "999.5k");
  assert.equal(formatTokens(999500), "1M");
  assert.equal(formatTokens(1500000), "1.5M");
  assert.equal(formatTokens(-5), "0");
});

test("money keeps tiered precision", () => {
  assert.equal(formatCny(0), `${CNY}0`);
  // The totals bar used to truncate this to ¥0.0012.
  assert.equal(formatCny(0.001203), `${CNY}0.001203`);
  assert.equal(formatCny(0.5), `${CNY}0.5000`);
  assert.equal(formatCny(93.7), `${CNY}93.70`);
});

test("token and money formatting exist in exactly one place", () => {
  const offenders = [];
  for (const file of sourceFiles(sourceRoot)) {
    if (file === formatPath) continue;
    const text = fs.readFileSync(file, "utf8");
    if (/function\s+formatTokens\b|const\s+formatTokens\s*=/.test(text)) {
      offenders.push(`${path.relative(sourceRoot, file)}:formatTokens`);
    }
    if (/function\s+formatCny\b|const\s+formatCny\s*=/.test(text)) {
      offenders.push(`${path.relative(sourceRoot, file)}:formatCny`);
    }
  }
  assert.deepEqual(offenders, []);
});
