#!/usr/bin/env node
"use strict";

// Run before Tauri acquires its Cargo build lock. Never invoke nested Cargo
// from build.rs, and never use an old sidecar as evidence of a successful build.
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const readline = require("node:readline");
const { spawn, spawnSync } = require("node:child_process");

const repoRoot = path.resolve(__dirname, "..", "..", "..");

function buildPlan(mode, env, host) {
  if (!["dev", "build"].includes(mode)) throw new Error("Expected sidecar mode: dev or build");
  const target = env.TAURI_ENV_TARGET_TRIPLE || host;
  if (!/^[a-zA-Z0-9_]+(?:-[a-zA-Z0-9_]+){2,}$/.test(target || "")) {
    throw new Error("Cannot determine a valid Rust target triple for the CLI sidecar");
  }
  if (target === "universal-apple-darwin") {
    throw new Error("Build the CLI sidecar for an explicit macOS architecture, not universal");
  }
  const release = mode === "build" && env.TAURI_ENV_DEBUG !== "true";
  const args = ["build", "-p", "deepagent-cli", "--bin", "deepagent", "--locked", "--target", target,
    "--message-format=json-render-diagnostics"];
  // Explicit --target also overrides Cargo config and CARGO_BUILD_TARGET, so a
  // differently configured host cannot silently package the wrong executable.
  if (release) args.push("--release");
  return { target, release, args };
}

function runCargo(args, { cwd, env }) {
  return new Promise((resolve, reject) => {
    const child = spawn(env.CARGO || "cargo", args, {
      cwd, env, windowsHide: true, stdio: ["ignore", "pipe", "inherit"],
    });
    let executable;
    const lines = readline.createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      let message;
      try { message = JSON.parse(line); } catch { process.stdout.write(line + "\n"); return; }
      if (message.reason === "compiler-artifact" && message.target?.name === "deepagent" &&
          message.target.kind?.includes("bin") && message.executable) {
        executable = message.executable;
      }
    });
    child.on("error", reject);
    child.on("close", (code, signal) => {
      if (code !== 0) reject(new Error(`CLI sidecar build failed (${signal || code})`));
      else if (!executable) reject(new Error("Cargo did not report a deepagent CLI executable"));
      else resolve(executable);
    });
  });
}

function digest(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

async function prepareSidecar({
  mode = "dev", root = repoRoot, env = process.env, host, build = runCargo,
} = {}) {
  if (!host) {
    const result = spawnSync(env.RUSTC || "rustc", ["-vV"], {
      env, encoding: "utf8", windowsHide: true,
    });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`Cannot query Rust host: ${result.stderr}`);
    host = /^host:\s*(\S+)/m.exec(result.stdout)?.[1];
  }
  const plan = buildPlan(mode, env, host);
  console.log(`[prepare-sidecar] building ${plan.target} (${plan.release ? "release" : "debug"})`);
  const executable = await build([
    ...plan.args, "--manifest-path", path.join(root, "Cargo.toml"),
  ], { cwd: root, env });
  const stat = fs.statSync(executable);
  if (!stat.isFile() || stat.size === 0) throw new Error("CLI executable is missing or empty");
  const extension = plan.target.includes("-windows-") ? ".exe" : "";
  const destination = path.join(root, "apps/desktop/src-tauri/binaries", `deepagent-${plan.target}${extension}`);
  const copied = !fs.existsSync(destination) || digest(executable) !== digest(destination);
  if (copied) {
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    const temporary = `${destination}.${process.pid}.tmp`;
    let ownsTemporary = false;
    try {
      const descriptor = fs.openSync(temporary, "wx", stat.mode & 0o777);
      ownsTemporary = true;
      fs.closeSync(descriptor);
      fs.copyFileSync(executable, temporary);
      fs.chmodSync(temporary, stat.mode & 0o777);
      fs.renameSync(temporary, destination);
    } finally {
      if (ownsTemporary && fs.existsSync(temporary)) fs.unlinkSync(temporary);
    }
  }
  console.log(`[prepare-sidecar] ${copied ? "prepared" : "unchanged"}: ${destination}`);
  return { ...plan, executable, destination, copied };
}

async function runTauri(args, {
  env = process.env, prepare = prepareSidecar,
  run = (values) => require("@tauri-apps/cli").run(values),
} = {}) {
  const separator = args.indexOf("--");
  const options = separator < 0 ? args : args.slice(0, separator);
  if (options[0] === "dev" && !options.some((arg) => ["-h", "--help", "-V", "--version"].includes(arg))) {
    let target;
    for (let i = 1; i < options.length; i++) {
      if (options[i] === "--target" || options[i] === "-t") target = options[++i];
      else if (options[i].startsWith("--target=")) target = options[i].slice("--target=".length);
    }
    // Build before entering Tauri: its async beforeDevCommand has a finite
    // dev-server wait, which a clean Rust build can legitimately exceed.
    await prepare({ mode: "build", env: { ...env,
      ...(target ? { TAURI_ENV_TARGET_TRIPLE: target } : {}),
      TAURI_ENV_DEBUG: options.includes("--release") ? "false" : "true",
    } });
  }
  await run(args);
}

module.exports = { buildPlan, prepareSidecar, runCargo, runTauri };
if (require.main === module) {
  const action = process.argv[2] === "tauri"
    ? runTauri(process.argv.slice(3))
    : prepareSidecar({ mode: process.argv[2] || "dev" });
  action.catch((error) => {
    console.error(`[prepare-sidecar] ${error.message}`);
    process.exitCode = 1;
  });
}
