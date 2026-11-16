"use strict";
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { runCargo } = require("./prepare-sidecar.cjs");

async function prepare({ root = path.resolve(__dirname, "../../.."), env = process.env, build = runCargo } = {}) {
  let target = env.TAURI_ENV_TARGET_TRIPLE;
  if (!target) {
    const result = spawnSync(env.RUSTC || "rustc", ["-vV"], { encoding: "utf8", windowsHide: true });
    if (result.status !== 0) throw new Error("Cannot identify helper target");
    target = /^host:\s*(\S+)/m.exec(result.stdout)?.[1];
  }
  if (!target || !/^[a-zA-Z0-9_-]+$/.test(target)) throw new Error("Invalid helper target");
  if (!target.includes("-windows-")) return { skipped: true };
  const args = ["build", "-p", "deepagent-installer", "--bin", "deepagent-install-options", "--locked", "--target", target,
    "--manifest-path", path.join(root, "Cargo.toml"), "--message-format=json-render-diagnostics"];
  if (env.TAURI_ENV_DEBUG !== "true") args.push("--release");
  const executable = await build(args, { cwd: root, env, binary: "deepagent-install-options" });
  const destination = path.join(root, "apps/desktop/src-tauri/binaries/deepagent-install-options.exe");
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.copyFileSync(executable, destination);
  console.log(`[prepare-installer-options] prepared ${target}`);
  return { destination, executable, args };
}
module.exports = { prepare };
if (require.main === module) prepare().catch(error => { console.error(error); process.exitCode = 1; });
