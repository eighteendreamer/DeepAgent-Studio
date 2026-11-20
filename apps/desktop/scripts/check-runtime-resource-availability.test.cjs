const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

const filename = path.resolve(__dirname, "../src/components/settings/RuntimeResourceSettings.tsx");
const source = fs.readFileSync(filename, "utf8");
const parsed = ts.createSourceFile(filename, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const availability = parsed.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "unavailableReason");
assert.ok(availability, "the resource page owns its availability decision");
const { outputText } = ts.transpileModule(availability.getText(parsed), {
  compilerOptions: { target: ts.ScriptTarget.ES2020 },
});
const unavailableReason = new Function(`${outputText}\nreturn unavailableReason;`)();

test("a supported resource can download without a pinned hash", () => {
  assert.equal(unavailableReason({ installed: false, available_for_platform: true, checksum_pinned: false }), undefined);
});

test("a supported resource with a pinned hash can still download", () => {
  assert.equal(unavailableReason({ installed: false, available_for_platform: true, checksum_pinned: true }), undefined);
});

test("an unsupported platform remains unavailable", () => {
  assert.equal(unavailableReason({ installed: false, available_for_platform: false, checksum_pinned: false }), "当前系统不可用");
});

test("an already installed resource stays available", () => {
  assert.equal(unavailableReason({ installed: true, available_for_platform: false, checksum_pinned: false }), undefined);
});
