#!/usr/bin/env node
"use strict";

/**
 * Compliance gate for bundled plugins.
 *
 * Plugins under `resources/plugins/` ship inside the application, so any
 * third-party plugin there carries a redistribution obligation that is ours to
 * satisfy. This script enforces four rules:
 *
 *   1. Every plugin directory is classified in `bundled-plugins.json` as one of
 *      the supported buckets. An unclassified plugin fails.
 *   2. Every bundled third-party entry keeps its declared license file in-tree.
 *   3. Every bundled third-party entry is named in `THIRD_PARTY_NOTICES.md`.
 *   4. Every bundled third-party entry records the bundled version and a
 *      deterministic content hash of the full plugin directory.
 *
 * Classification is explicit rather than inferred from each manifest: whether a
 * plugin is ours or vendored is a distribution decision, and a manifest
 * `author` field can be edited while the obligation cannot.
 *
 * Exit code 0 on success, 1 on any violation.
 */

const fs = require("node:fs");
const crypto = require("node:crypto");
const path = require("node:path");

const desktopRoot = path.resolve(__dirname, "..");
const repoRoot = path.resolve(desktopRoot, "..", "..");
const pluginsDir = path.join(desktopRoot, "src-tauri", "resources", "plugins");
const manifestPath = path.join(desktopRoot, "src-tauri", "bundled-plugins.json");
const noticesPath = path.join(repoRoot, "THIRD_PARTY_NOTICES.md");

/** Names under `resources/plugins/` that are service directories, not plugins. */
const NON_PLUGIN_ENTRIES = new Set(["cache", "marketplaces", "data"]);

const problems = [];

function fail(message) {
  problems.push(message);
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (error) {
    fail(`cannot read ${path.relative(repoRoot, file)}: ${error.message}`);
    return null;
  }
}

function listPluginDirs() {
  if (!fs.existsSync(pluginsDir)) {
    fail(`bundled plugin directory is missing: ${path.relative(repoRoot, pluginsDir)}`);
    return [];
  }
  return fs
    .readdirSync(pluginsDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && !NON_PLUGIN_ENTRIES.has(entry.name))
    .map((entry) => entry.name)
    .sort();
}

function main() {
  const manifest = readJson(manifestPath);
  const notices = fs.existsSync(noticesPath)
    ? fs.readFileSync(noticesPath, "utf8")
    : (fail(`missing ${path.relative(repoRoot, noticesPath)}`), "");

  if (!manifest) {
    report();
    return;
  }

  const firstParty = Array.isArray(manifest.firstParty) ? manifest.firstParty : [];
  const bundledThirdParty = Array.isArray(manifest.bundledThirdParty)
    ? manifest.bundledThirdParty
    : [];
  const marketplaceOnly = Array.isArray(manifest.marketplaceOnly) ? manifest.marketplaceOnly : [];

  if (!Array.isArray(manifest.firstParty)) {
    fail("bundled-plugins.json: `firstParty` must be an array");
  }
  if (!Array.isArray(manifest.bundledThirdParty)) {
    fail("bundled-plugins.json: `bundledThirdParty` must be an array");
  }
  if (!Array.isArray(manifest.marketplaceOnly)) {
    fail("bundled-plugins.json: `marketplaceOnly` must be an array");
  }

  const classified = new Map();
  for (const name of firstParty) {
    if (classified.has(name)) {
      fail(`bundled-plugins.json: '${name}' is listed more than once`);
    }
    classified.set(name, "firstParty");
  }

  for (const entry of bundledThirdParty) {
    validateBundledEntry(entry, "bundledThirdParty", classified, notices);
  }

  for (const entry of marketplaceOnly) {
    validateMarketplaceOnlyEntry(entry);
  }

  const present = listPluginDirs();
  for (const name of present) {
    if (!classified.has(name)) {
      fail(
        `plugin '${name}' is bundled but not classified in ` +
          `${path.relative(repoRoot, manifestPath)}. Add it to \`firstParty\` if this project ` +
          `authored it, or to \`bundledThirdParty\` with its upstream and license, or to ` +
          `\`marketplaceOnly\` if it should only be installed from the marketplace.`,
      );
    }
  }

  for (const name of classified.keys()) {
    if (!present.includes(name)) {
      fail(
        `bundled-plugins.json lists '${name}', but no such plugin directory exists under ` +
          `${path.relative(repoRoot, pluginsDir)}`,
      );
    }
  }

  report(present.length, classified);
}

function validateBundledEntry(entry, bucket, classified, notices) {
  if (!entry || typeof entry.name !== "string" || entry.name === "") {
    fail(`bundled-plugins.json: every \`${bucket}\` entry needs a non-empty \`name\``);
    return;
  }
  if (classified.has(entry.name)) {
    fail(`bundled-plugins.json: '${entry.name}' is listed more than once`);
  }
  classified.set(entry.name, bucket);

  for (const field of ["upstream", "license", "licenseFile", "version", "contentHash"]) {
    if (typeof entry[field] !== "string" || entry[field] === "") {
      fail(`bundled-plugins.json: '${entry.name}' is missing \`${field}\``);
    }
  }

  const pluginDir = path.join(pluginsDir, entry.name);
  const manifestVersion = readPluginManifestVersion(pluginDir);
  if (
    typeof entry.version === "string" &&
    manifestVersion &&
    entry.version !== manifestVersion
  ) {
    fail(
      `'${entry.name}' declares bundled version ${entry.version}, but its plugin manifest ` +
        `declares ${manifestVersion}`,
    );
  }

  if (typeof entry.contentHash === "string" && entry.contentHash !== "") {
    if (!/^sha256:[0-9a-f]{64}$/.test(entry.contentHash)) {
      fail(`'${entry.name}': contentHash must be sha256:<64 lowercase hex chars>`);
    } else if (fs.existsSync(pluginDir)) {
      const actualHash = pluginDirectoryContentHash(pluginDir);
      if (entry.contentHash !== actualHash) {
        fail(
          `'${entry.name}' content hash changed: expected ${entry.contentHash}, got ` +
            `${actualHash}. Update bundled-plugins.json and THIRD_PARTY_NOTICES.md only ` +
            `after reviewing the upstream source and license.`,
        );
      }
    }
  }

  if (typeof entry.licenseFile === "string" && entry.licenseFile !== "") {
    const licensePath = path.join(pluginDir, entry.licenseFile);
    if (!fs.existsSync(licensePath)) {
      fail(
        `'${entry.name}' is bundled third-party (${entry.license ?? "unknown license"}) ` +
          `but its license file is missing: ${path.relative(repoRoot, licensePath)}`,
      );
    } else if (fs.statSync(licensePath).size === 0) {
      fail(`'${entry.name}': license file is empty: ${path.relative(repoRoot, licensePath)}`);
    }
  }

  if (notices && !notices.includes(entry.name)) {
    fail(
      `'${entry.name}' is bundled third-party but is not recorded in ` +
        `${path.relative(repoRoot, noticesPath)}`,
    );
  }
  for (const field of ["upstream", "version", "license", "contentHash"]) {
    if (
      typeof entry[field] === "string" &&
      entry[field] !== "" &&
      !notices.includes(entry[field])
    ) {
      fail(
        `'${entry.name}' ${field} '${entry[field]}' is missing from ` +
          `${path.relative(repoRoot, noticesPath)}`,
      );
    }
  }
}

function validateMarketplaceOnlyEntry(entry) {
  if (!entry || typeof entry.name !== "string" || entry.name === "") {
    fail("bundled-plugins.json: every `marketplaceOnly` entry needs a non-empty `name`");
    return;
  }
  for (const field of ["upstream", "license"]) {
    if (typeof entry[field] !== "string" || entry[field] === "") {
      fail(`bundled-plugins.json: marketplace-only '${entry.name}' is missing \`${field}\``);
    }
  }
}

function readPluginManifestVersion(pluginDir) {
  for (const manifest of [
    path.join(pluginDir, ".codex-plugin", "plugin.json"),
    path.join(pluginDir, ".claude-plugin", "plugin.json"),
  ]) {
    if (!fs.existsSync(manifest)) {
      continue;
    }
    try {
      const parsed = JSON.parse(fs.readFileSync(manifest, "utf8"));
      return typeof parsed.version === "string" && parsed.version !== "" ? parsed.version : null;
    } catch (error) {
      fail(`cannot parse ${path.relative(repoRoot, manifest)}: ${error.message}`);
      return null;
    }
  }
  fail(
    `'${path.basename(pluginDir)}' is missing .codex-plugin/plugin.json or ` +
      `.claude-plugin/plugin.json`,
  );
  return null;
}

function pluginDirectoryContentHash(root) {
  const entries = [];
  collectPluginHashEntries(root, root, entries);
  entries.sort((a, b) => {
    if (a.relative < b.relative) return -1;
    if (a.relative > b.relative) return 1;
    if (a.kind < b.kind) return -1;
    if (a.kind > b.kind) return 1;
    return 0;
  });

  const hasher = crypto.createHash("sha256");
  hasher.update(Buffer.from("deepagent-plugin-dir-v1\0"));
  for (const entry of entries) {
    hasher.update(entry.kind);
    hasher.update(Buffer.from("\0"));
    hasher.update(entry.relative);
    hasher.update(Buffer.from("\0"));
    if (entry.kind === "file") {
      const stat = fs.statSync(entry.absolute);
      hasher.update(String(stat.size));
      hasher.update(Buffer.from("\0"));
      hasher.update(fs.readFileSync(entry.absolute));
    }
    hasher.update(Buffer.from("\0"));
  }
  return `sha256:${hasher.digest("hex")}`;
}

function collectPluginHashEntries(root, current, entries) {
  for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
    if (entry.isDirectory() && entry.name === ".git") {
      continue;
    }
    const absolute = path.join(current, entry.name);
    const relative = path.relative(root, absolute).split(path.sep).join("/");
    if (entry.isDirectory()) {
      entries.push({ kind: "dir", relative, absolute });
      collectPluginHashEntries(root, absolute, entries);
    } else if (entry.isFile()) {
      entries.push({ kind: "file", relative, absolute });
    }
  }
}

function report(pluginCount, classified) {
  if (problems.length > 0) {
    console.error("[check-plugin-licenses] compliance problems:");
    for (const problem of problems) {
      console.error(`  - ${problem}`);
    }
    console.error(
      "\nSee THIRD_PARTY_NOTICES.md for the recording rules. Anthropic's plugins from " +
        "anthropics/claude-code must not be bundled: that repository grants no " +
        "redistribution right.",
    );
    process.exit(1);
  }

  const bundledThirdPartyCount = classified
    ? [...classified.values()].filter((kind) => kind === "bundledThirdParty").length
    : 0;
  console.log(
    `[check-plugin-licenses] ${pluginCount} bundled plugins checked ` +
      `(${bundledThirdPartyCount} bundled third-party, all licensed and recorded)`,
  );
}

main();
