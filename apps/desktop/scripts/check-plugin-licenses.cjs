#!/usr/bin/env node
"use strict";

/**
 * Compliance gate for bundled plugins.
 *
 * Plugins under `resources/plugins/` ship inside the application, so any
 * third-party plugin there carries a redistribution obligation that is ours to
 * satisfy. This script enforces three rules:
 *
 *   1. Every plugin directory is classified in `bundled-plugins.json` as either
 *      first-party or third-party. An unclassified plugin fails — that is what
 *      stops a vendored plugin from slipping in unrecorded.
 *   2. Every third-party entry keeps its declared license file in-tree.
 *   3. Every third-party entry is named in `THIRD_PARTY_NOTICES.md`.
 *
 * Classification is explicit rather than inferred from each manifest: whether a
 * plugin is ours or vendored is a distribution decision, and a manifest `author`
 * field can be edited while the obligation cannot.
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
  const thirdParty = Array.isArray(manifest.thirdParty) ? manifest.thirdParty : [];

  if (!Array.isArray(manifest.firstParty)) {
    fail("bundled-plugins.json: `firstParty` must be an array");
  }
  if (!Array.isArray(manifest.thirdParty)) {
    fail("bundled-plugins.json: `thirdParty` must be an array");
  }

  const classified = new Map();
  for (const name of firstParty) {
    if (classified.has(name)) {
      fail(`bundled-plugins.json: '${name}' is listed more than once`);
    }
    classified.set(name, "firstParty");
  }

  for (const entry of thirdParty) {
    if (!entry || typeof entry.name !== "string" || entry.name === "") {
      fail("bundled-plugins.json: every `thirdParty` entry needs a non-empty `name`");
      continue;
    }
    if (classified.has(entry.name)) {
      fail(`bundled-plugins.json: '${entry.name}' is listed more than once`);
    }
    classified.set(entry.name, "thirdParty");

    for (const field of ["upstream", "license", "licenseFile"]) {
      if (typeof entry[field] !== "string" || entry[field] === "") {
        fail(`bundled-plugins.json: '${entry.name}' is missing \`${field}\``);
      }
    }

    // Rule 2: the license text must ship with the plugin.
    if (typeof entry.licenseFile === "string" && entry.licenseFile !== "") {
      const licensePath = path.join(pluginsDir, entry.name, entry.licenseFile);
      if (!fs.existsSync(licensePath)) {
        fail(
          `'${entry.name}' is third-party (${entry.license ?? "unknown license"}) but its ` +
            `license file is missing: ${path.relative(repoRoot, licensePath)}`,
        );
      } else if (fs.statSync(licensePath).size === 0) {
        fail(`'${entry.name}': license file is empty: ${path.relative(repoRoot, licensePath)}`);
      }
    }

    // Rule 3: it must be recorded for downstream recipients.
    if (notices && !notices.includes(entry.name)) {
      fail(
        `'${entry.name}' is third-party but is not recorded in ` +
          `${path.relative(repoRoot, noticesPath)}`,
      );
    }
  }

  // Rule 1: no unclassified plugin, and no stale classification.
  const present = listPluginDirs();
  for (const name of present) {
    if (!classified.has(name)) {
      fail(
        `plugin '${name}' is bundled but not classified in ` +
          `${path.relative(repoRoot, manifestPath)}. Add it to \`firstParty\` if this project ` +
          `authored it, or to \`thirdParty\` with its upstream and license.`,
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

  const thirdPartyCount = classified
    ? [...classified.values()].filter((kind) => kind === "thirdParty").length
    : 0;
  console.log(
    `[check-plugin-licenses] ${pluginCount} bundled plugins checked ` +
      `(${thirdPartyCount} third-party, all licensed and recorded)`,
  );
}

main();
