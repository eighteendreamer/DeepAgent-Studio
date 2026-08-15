#!/usr/bin/env node
"use strict";

/**
 * Compliance gate for bundled plugins.
 *
 * Plugins under `resources/plugins/` ship inside the application, so any
 * third-party plugin there carries a redistribution obligation that is ours to
 * satisfy. This script enforces three rules:
 *
 *   1. Every plugin directory is classified in `bundled-plugins.json` as one of
 *      the supported buckets. An unclassified plugin fails.
 *   2. Every bundled third-party entry keeps its declared license file in-tree.
 *   3. Every bundled third-party entry is named in `THIRD_PARTY_NOTICES.md`.
 *
 * Classification is explicit rather than inferred from each manifest: whether a
 * plugin is ours or vendored is a distribution decision, and a manifest
 * `author` field can be edited while the obligation cannot.
 *
 * Exit code 0 on success, 1 on any violation.
 */

const fs = require("node:fs");
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

  for (const field of ["upstream", "license", "licenseFile"]) {
    if (typeof entry[field] !== "string" || entry[field] === "") {
      fail(`bundled-plugins.json: '${entry.name}' is missing \`${field}\``);
    }
  }

  if (typeof entry.licenseFile === "string" && entry.licenseFile !== "") {
    const licensePath = path.join(pluginsDir, entry.name, entry.licenseFile);
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
}

function validateMarketplaceOnlyEntry(entry) {
  if (!entry || typeof entry.name !== "string" || entry.name === "") {
    fail("bundled-plugins.json: every `marketplaceOnly` entry needs a non-empty `name`");
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
