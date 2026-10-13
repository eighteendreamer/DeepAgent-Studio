#!/usr/bin/env node
"use strict";

// The existing REA packager builds both the standalone ZIP and the built-in
// payload. Source metadata lives in resources/plugins; generated ZIPs stay out
// of Git. This first release only ships a verified Windows x64 runtime.
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");

const repoRoot = path.resolve(__dirname, "..", "..", "..");
const pluginRoot = path.join(repoRoot, "apps/desktop/src-tauri/resources/plugins/rea");
const runtimeRoot = path.join(repoRoot, "plugins/rea/runtime");
const archivePath = path.join(pluginRoot, "runtime.zip");
const metadataPath = archivePath + ".json";

function hash(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function main() {
  if (process.platform !== "win32" || process.arch !== "x64") {
    console.log("[prebundle-rea] verified runtime is Windows x64 only; no payload generated for this platform");
    return;
  }
  const manifest = JSON.parse(fs.readFileSync(path.join(pluginRoot, ".codex-plugin/plugin.json"), "utf8"));
  const lockPath = path.join(runtimeRoot, "package-lock.json");
  const packagePath = path.join(runtimeRoot, "package.json");
  const lock = JSON.parse(fs.readFileSync(lockPath, "utf8"));
  const nodeVersion = manifest.runtime.node.replace(/^=/, "");
  if (fs.existsSync(archivePath) && fs.existsSync(metadataPath)) {
    try {
      const metadata = JSON.parse(fs.readFileSync(metadataPath, "utf8"));
      if (metadata.platform === "windows-x64" && metadata.nodeVersion === nodeVersion &&
          metadata.reaVersion === lock.packages["node_modules/rea-agents"].version &&
          metadata.pluginVersion === manifest.version && metadata.lockSha256 === hash(lockPath) &&
          metadata.packageSha256 === hash(packagePath) && metadata.runtimeSha256 === hash(archivePath)) {
        console.log("[prebundle-rea] built-in runtime verified; reusing payload");
        return;
      }
    } catch (error) {
      console.log(`[prebundle-rea] payload metadata invalid: ${error.message}; rebuilding`);
    }
  }
  const home = process.env.HOME || process.env.USERPROFILE || os.homedir();
  const dataRoot = process.env.DEEPAGENT_DATA_ROOT || path.join(home, ".deepagent");
  const managedNode = path.join(dataRoot, "files/runtimes/sdk/node-24", `node-v${nodeVersion}-win-x64`, "node.exe");
  const candidates = [...new Set([process.execPath, managedNode])];
  const node = candidates.find((candidate) => {
    if (!fs.existsSync(candidate)) return false;
    const result = spawnSync(candidate, ["-p", 'process.platform + "/" + process.arch + "/" + process.version'],
      { encoding: "utf8", windowsHide: true });
    return result.status === 0 && result.stdout.trim() === `win32/x64/v${nodeVersion}`;
  });
  if (!node) {
    throw new Error(`REA preparation needs Windows x64 Node ${nodeVersion}. Install the managed Node SDK or run the desktop build with that Node version.`);
  }
  const result = spawnSync("pwsh", ["-NoProfile", "-File", path.join(repoRoot, "scripts/package-rea-plugin.ps1"), "-NodePath", node],
    { cwd: repoRoot, stdio: "inherit", windowsHide: true });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`REA packager failed with exit code ${result.status}`);
}

main();
