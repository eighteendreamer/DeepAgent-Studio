const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { stage, parseInstallFiles } = require("./stage-payload.cjs");
const { prepare } = require("../../desktop/scripts/prepare-installer-options.cjs");
const repo = path.resolve(__dirname, "../../..");

test("helper builds the requested target; non-Windows packaging stays unchanged", async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "deepagent-helper-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const exe = path.join(root, "helper.exe"); fs.writeFileSync(exe, "fixture");
  let builds = 0;
  const build = async (args, options) => {
    builds++; assert.equal(options.binary, "deepagent-install-options");
    assert.ok(args.includes("--locked")); assert.ok(args.includes("--release"));
    assert.equal(args[args.indexOf("--target") + 1], "x86_64-pc-windows-msvc"); return exe;
  };
  await prepare({ root, build, env: { TAURI_ENV_TARGET_TRIPLE: "x86_64-pc-windows-msvc" } });
  await prepare({ root, build, env: { TAURI_ENV_TARGET_TRIPLE: "aarch64-apple-darwin" } });
  assert.equal(builds, 1);
});

function nsisSource(root, directive = 'File "${MAINBINARYSRCPATH}"') {
  return `!define PRODUCTNAME "DeepAgent Studio"\n!define MAINBINARYNAME "deepagent-desktop"\n!define INSTALLMODE "currentUser"\n!define VERSION "0.0.8"\n!define MAINBINARYSRCPATH "${path.join(root, "main.exe")}"\nSection Install\n${directive}\nFile /a "/oname=deepagent.exe" "${path.join(root, "cli.exe")}"\nSectionEnd\n`;
}
test("staging uses the actual NSIS file list and digests; malformed/stale packages fail closed", async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "deepagent-stage-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const name of ["main.exe", "cli.exe", "helper.exe", "nsis.exe"]) fs.writeFileSync(path.join(root, name), "fixture " + name);
  const script = path.join(root, "installer.nsi"); fs.writeFileSync(script, nsisSource(root));
  const input = { nsis: path.join(root, "nsis.exe"), script, helper: path.join(root, "helper.exe"), output: path.join(root, "out"), version: "0.0.8" };
  const manifest = await stage(input);
  assert.equal(manifest.files.length, 3); assert.equal(manifest.setup_sha256.length, 64);
  assert.equal(manifest.installed_bytes, manifest.files.reduce((n,f) => n + f.bytes, 0));
  await assert.rejects(stage({ ...input, version: "0.0.9" }), /version/);
  assert.throws(() => parseInstallFiles(nsisSource(root, 'File /r "folder"')), /Unrecognized/);
  fs.writeFileSync(script, nsisSource(root, `File /a "/oname=../escape" "${path.join(root,"main.exe")}"`));
  await assert.rejects(stage(input), /Unsafe/);
});
test("configuration keeps the updater on NSIS and enables only the installer's window APIs", () => {
  const desktop = JSON.parse(fs.readFileSync(path.join(repo, "apps/desktop/src-tauri/tauri.conf.json")));
  assert.equal(desktop.bundle.windows.nsis.installMode, "currentUser");
  assert.deepEqual(desktop.bundle.windows.nsis.languages, ["SimpChinese", "English"]);
  const capabilities = JSON.parse(fs.readFileSync(path.join(repo, "apps/installer/src-tauri/capabilities/default.json")));
  assert.deepEqual(capabilities.windows, ["installer"]);
  assert.ok(!capabilities.permissions.some(p => /shell|fs:|opener|updater/.test(p)));
  const workflow = fs.readFileSync(path.join(repo, ".github/workflows/release-desktop.yml"), "utf8");
  assert.match(workflow, /includeUpdaterJson: true/); assert.match(workflow, /updaterJsonPreferNsis: true/);
  assert.doesNotMatch(workflow, /uploadUpdaterJson:|releaseAssetNamePattern:/);
});
