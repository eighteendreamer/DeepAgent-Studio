const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { buildPlan, prepareSidecar, runCargo, runTauri } = require("./prepare-sidecar.cjs");

const host = "x86_64-pc-windows-msvc";

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "deepagent-sidecar-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const executable = path.join(root, "custom target directory", "deepagent.exe");
  fs.mkdirSync(path.dirname(executable), { recursive: true });
  fs.writeFileSync(executable, "synthetic executable");
  return { root, executable };
}

test("Tauri prepares the required CLI before both development and packaging", () => {
  const desktop = path.resolve(__dirname, "..");
  const config = JSON.parse(fs.readFileSync(path.join(desktop, "src-tauri/tauri.conf.json"), "utf8"));
  const pkg = JSON.parse(fs.readFileSync(path.join(desktop, "package.json"), "utf8"));
  assert.deepEqual(config.bundle.externalBin, ["binaries/deepagent"]);
  assert.equal(pkg.scripts.tauri, "node scripts/prepare-sidecar.cjs tauri");
  assert.equal(config.build.beforeDevCommand, "pnpm dev");
  assert.match(config.build.beforeBuildCommand, /^node scripts\/prepare-sidecar\.cjs build && /);
});

test("dev awaits preparation before Tauri's server wait; target/release and app arguments stay distinct", async () => {
  for (const [args, target, debug] of [
    [["dev"], undefined, "true"],
    [["dev", "--release", "--target", "aarch64-apple-darwin"], "aarch64-apple-darwin", "false"],
    [["dev", "-t", host, "--", "--release"], host, "true"],
    [["dev", `--target=${host}`], host, "true"],
  ]) {
    const events = [];
    await runTauri(args, { env: {}, prepare: async (options) => {
      await Promise.resolve();
      assert.equal(options.env.TAURI_ENV_TARGET_TRIPLE, target);
      assert.equal(options.env.TAURI_ENV_DEBUG, debug);
      events.push("prepared");
    }, run: async (actual) => { assert.deepEqual(actual, args); events.push("tauri"); } });
    assert.deepEqual(events, ["prepared", "tauri"]);
  }
  let invoked = false;
  await assert.rejects(runTauri(["dev"], { prepare: async () => { throw new Error("build failed"); },
    run: async () => { invoked = true; } }), /build failed/);
  assert.equal(invoked, false);
});

test("help, info and build pass through; build preparation stays in the target-aware hook", async () => {
  for (const args of [["dev", "--help"], ["--help"], ["info"], ["build", "--target", "aarch64-apple-darwin"]]) {
    let invoked = false;
    await runTauri(args, { prepare: async () => assert.fail("unexpected preparation"),
      run: async (actual) => { invoked = true; assert.deepEqual(actual, args); } });
    assert.equal(invoked, true);
  }
});

test("debug/release and cross-platform targets follow the Tauri hook environment", () => {
  for (const [mode, env, release, target] of [
    ["dev", {}, false, host],
    ["build", {}, true, host],
    ["build", { TAURI_ENV_DEBUG: "false" }, true, host],
    ["build", { TAURI_ENV_DEBUG: "true" }, false, host],
    ["build", { TAURI_ENV_TARGET_TRIPLE: "aarch64-apple-darwin" }, true, "aarch64-apple-darwin"],
    ["dev", { TAURI_ENV_TARGET_TRIPLE: "x86_64-unknown-linux-gnu" }, false, "x86_64-unknown-linux-gnu"],
    ["dev", { CARGO_BUILD_TARGET: "aarch64-apple-darwin" }, false, host],
  ]) {
    const plan = buildPlan(mode, env, host);
    assert.equal(plan.release, release);
    assert.equal(plan.args.includes("--release"), release);
    assert.equal(plan.target, target);
    assert.equal(plan.args[plan.args.indexOf("--target") + 1], target);
    assert.ok(plan.args.includes("--locked"));
  }
  assert.throws(() => buildPlan("unknown", {}, host), /mode/);
  assert.throws(() => buildPlan("dev", { TAURI_ENV_TARGET_TRIPLE: "../../escape" }, host), /target triple/);
  assert.throws(() => buildPlan("build", { TAURI_ENV_TARGET_TRIPLE: "universal-apple-darwin" }, host), /explicit macOS/);
});

test("a clean checkout uses Cargo's reported artifact and creates the target-suffixed sidecar", async (t) => {
  const { root, executable } = fixture(t);
  let calls = 0;
  const result = await prepareSidecar({ root, host, env: {}, build: async (args, options) => {
    calls++;
    assert.equal(options.cwd, root);
    assert.equal(args[args.indexOf("--manifest-path") + 1], path.join(root, "Cargo.toml"));
    return executable;
  } });
  assert.equal(calls, 1);
  assert.equal(result.destination, path.join(root, `apps/desktop/src-tauri/binaries/deepagent-${host}.exe`));
  assert.equal(result.copied, true);
  assert.equal(fs.readFileSync(result.destination, "utf8"), "synthetic executable");
});

test("each preparation checks Cargo freshness; equal output is not rewritten and changed output replaces it", async (t) => {
  const { root, executable } = fixture(t);
  let calls = 0;
  const options = { root, host, env: {}, build: async () => { calls++; return executable; } };
  const first = await prepareSidecar(options);
  const fixedTime = new Date("2020-01-01T00:00:00Z");
  fs.utimesSync(first.destination, fixedTime, fixedTime);
  assert.equal((await prepareSidecar(options)).copied, false);
  assert.equal(fs.statSync(first.destination).mtimeMs, fixedTime.getTime());
  fs.writeFileSync(executable, "new CLI build");
  assert.equal((await prepareSidecar(options)).copied, true);
  assert.equal(fs.readFileSync(first.destination, "utf8"), "new CLI build");
  assert.equal(calls, 3);
});

test("build failure never reuses an old sidecar or changes it", async (t) => {
  const { root, executable } = fixture(t);
  const first = await prepareSidecar({ root, host, env: {}, build: async () => executable });
  await assert.rejects(prepareSidecar({ root, host, env: {}, build: async () => {
    throw new Error("synthetic Cargo failure");
  } }), /synthetic Cargo failure/);
  assert.equal(fs.readFileSync(first.destination, "utf8"), "synthetic executable");
});

test("non-Windows targets have no exe suffix and retain executable permissions", async (t) => {
  const { root, executable } = fixture(t);
  fs.chmodSync(executable, 0o755);
  const result = await prepareSidecar({ root, host, env: { TAURI_ENV_TARGET_TRIPLE: "aarch64-apple-darwin" },
    mode: "build", build: async () => executable });
  assert.equal(path.basename(result.destination), "deepagent-aarch64-apple-darwin");
  if (process.platform !== "win32") assert.equal(fs.statSync(result.destination).mode & 0o777, 0o755);
});

test("an empty artifact and an existing temporary file fail without inventing a sidecar", async (t) => {
  const { root, executable } = fixture(t);
  fs.writeFileSync(executable, "");
  await assert.rejects(prepareSidecar({ root, host, env: {}, build: async () => executable }), /empty/);
  fs.writeFileSync(executable, "synthetic executable");
  const directory = path.join(root, "apps/desktop/src-tauri/binaries");
  fs.mkdirSync(directory, { recursive: true });
  const temporary = path.join(directory, `deepagent-${host}.exe.${process.pid}.tmp`);
  fs.writeFileSync(temporary, "existing file must survive");
  await assert.rejects(prepareSidecar({ root, host, env: {}, build: async () => executable }), /EEXIST/);
  assert.equal(fs.readFileSync(temporary, "utf8"), "existing file must survive");
});

test("the Cargo JSON reader accepts only the CLI bin and preserves subprocess failures", async (t) => {
  const { root, executable } = fixture(t);
  const writer = path.join(root, "cargo-fixture.cjs");
  fs.writeFileSync(writer, `
    const mode=process.argv[2];
    for (const event of [
      {reason:"compiler-artifact",target:{name:"deepagent",kind:["lib"]},executable:"wrong-library"},
      {reason:"compiler-artifact",target:{name:"other",kind:["bin"]},executable:"wrong-bin"},
      ...(mode==="missing"?[]:[{reason:"compiler-artifact",target:{name:"deepagent",kind:["bin"]},executable:process.argv[3]}])
    ]) console.log(JSON.stringify(event));
    if(mode==="failure") process.exitCode=9;
  `);
  const options = { cwd: root, env: { ...process.env, CARGO: process.execPath } };
  assert.equal(await runCargo([writer, "success", executable], options), executable);
  await assert.rejects(runCargo([writer, "missing", executable], options), /did not report/);
  await assert.rejects(runCargo([writer, "failure", executable], options), /failed \(9\)/);
});
