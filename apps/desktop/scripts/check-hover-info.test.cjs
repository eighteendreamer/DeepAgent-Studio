const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

const sourceRoot = path.resolve(__dirname, "../src");

function sourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(directory, entry.name);
    return entry.isDirectory() ? sourceFiles(entryPath) : entry.name.endsWith(".tsx") ? [entryPath] : [];
  });
}

test("visible hover information does not use native title tooltips", () => {
  const offenders = [];
  for (const file of sourceFiles(sourceRoot)) {
    const source = fs.readFileSync(file, "utf8");
    const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    function visit(node) {
      if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
        const tag = node.tagName.getText(ast);
        if (tag === "title") offenders.push(`${path.relative(sourceRoot, file)}:${ast.getLineAndCharacterOfPosition(node.getStart()).line + 1}:svg-title`);
        if (/^[a-z]/.test(tag) && tag !== "iframe" && node.attributes.properties.some(
          (attribute) => ts.isJsxAttribute(attribute) && attribute.name.text === "title",
        )) offenders.push(`${path.relative(sourceRoot, file)}:${ast.getLineAndCharacterOfPosition(node.getStart()).line + 1}:${tag}`);
      }
      ts.forEachChild(node, visit);
    }
    visit(ast);
  }
  assert.deepEqual(offenders, []);
});

test("shared Hover Card uses the requested shadcn Base UI primitive", () => {
  const card = fs.readFileSync(path.join(sourceRoot, "components/shadcn/hover-card.tsx"), "utf8");
  assert.match(card, /@base-ui\/react\/preview-card/);
  assert.doesNotMatch(card, /@radix-ui\/react-hover-card/);
});
