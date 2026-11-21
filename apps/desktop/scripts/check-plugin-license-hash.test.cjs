const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { execFileSync } = require("node:child_process");
const { pluginDirectoryContentHash } = require("./check-plugin-licenses.cjs");

test("generated payload exclusion preserves the source integrity check", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "plugin-hash-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.writeFileSync(path.join(root, "README.md"), "original source");
  const excluded = new Set(["runtime.zip", "runtime.zip.json"]);
  const sourceHash = pluginDirectoryContentHash(root, excluded);
  fs.writeFileSync(path.join(root, "runtime.zip"), "generated output");
  fs.writeFileSync(path.join(root, "runtime.zip.json"), "generated metadata");
  assert.equal(pluginDirectoryContentHash(root, excluded), sourceHash);
  assert.notEqual(pluginDirectoryContentHash(root), sourceHash);
  fs.writeFileSync(path.join(root, "README.md"), "changed source");
  assert.notEqual(pluginDirectoryContentHash(root, excluded), sourceHash);
});

for (const autocrlf of ["true", "false"]) {
  test(`WEDecode checkout preserves its recorded hash with core.autocrlf=${autocrlf}`, (t) => {
    const repoRoot = path.resolve(__dirname, "..", "..", "..");
    const pluginPath = "apps/desktop/src-tauri/resources/plugins/wedecode";
    const manifest = JSON.parse(fs.readFileSync(
      path.join(repoRoot, "apps/desktop/src-tauri/bundled-plugins.json"), "utf8",
    ));
    const recorded = manifest.bundledThirdParty.find((entry) => entry.name === "wedecode");
    assert.ok(recorded, "WEDecode must retain its reviewed source hash");
    const files = execFileSync("git", ["ls-files", "-z", "--", pluginPath], {
      cwd: repoRoot, encoding: "utf8",
    }).split("\0").filter(Boolean);
    assert.ok(files.length > 0, "exercise the actual tracked plugin bundle");
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "plugin-checkout-"));
    t.after(() => fs.rmSync(root, { recursive: true, force: true }));
    execFileSync("git", [
      "-c", `core.autocrlf=${autocrlf}`, "checkout-index",
      `--prefix=${root.split(path.sep).join("/")}/`, "--", ...files,
    ], { cwd: repoRoot, stdio: "pipe" });
    const checkedOut = path.join(root, pluginPath);
    assert.equal(pluginDirectoryContentHash(checkedOut), recorded.contentHash);
    fs.appendFileSync(path.join(checkedOut, "commands/unpack.md"), "\nchanged source\n");
    assert.notEqual(pluginDirectoryContentHash(checkedOut), recorded.contentHash,
      "source changes must still fail the integrity check");
  });
}
