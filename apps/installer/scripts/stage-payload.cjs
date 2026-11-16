"use strict";
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
async function hash(file) {
  const sum = crypto.createHash("sha256");
  for await (const chunk of fs.createReadStream(file)) sum.update(chunk);
  return sum.digest("hex");
}
function parseInstallFiles(source) {
  const defines = Object.fromEntries([...source.matchAll(/^!define (\w+) "([^"\r\n]*)"/gm)].map(m => [m[1], m[2]]));
  if (defines.PRODUCTNAME !== "DeepAgent Studio" || defines.MAINBINARYNAME !== "deepagent-desktop" || defines.INSTALLMODE !== "currentUser") {
    throw new Error("Unexpected NSIS product, executable or installation scope");
  }
  const section = /^Section Install\r?\n([\s\S]*?)^SectionEnd/m.exec(source)?.[1];
  if (!section) throw new Error("Generated NSIS Install section is missing");
  const files = [];
  for (const line of section.split(/\r?\n/).map(s => s.trim()).filter(s => /^File\s/.test(s))) {
    if (line === 'File "${MAINBINARYSRCPATH}"') {
      files.push({ path: "deepagent-desktop.exe", source: defines.MAINBINARYSRCPATH });
    } else {
      const match = /^File \/a "\/oname=([^"$]+)" "([^"$]+)"$/.exec(line);
      if (!match) throw new Error(`Unrecognized NSIS file directive: ${line}`);
      files.push({ path: match[1].replace(/\\+/g, "/"), source: match[2] });
    }
  }
  if (!files.length) throw new Error("Empty NSIS file list");
  return { version: defines.VERSION, files };
}
async function stage({ nsis, script, helper, output, version }) {
  const parsed = parseInstallFiles(fs.readFileSync(script, "utf8"));
  if (parsed.version !== version) throw new Error("NSIS version differs from the bootstrapper");
  parsed.files.push({ path: "deepagent-install-options.exe", source: helper });
  const seen = new Set();
  const files = [];
  for (const file of parsed.files) {
    if (!file.path || path.posix.isAbsolute(file.path) || file.path.split("/").some(p => ["", ".", ".."].includes(p)) || file.path.includes(":")) throw new Error("Unsafe packaged path");
    const key = file.path.toLowerCase();
    if (seen.has(key)) throw new Error("Duplicate packaged file");
    seen.add(key);
    if (!fs.statSync(file.source).isFile()) throw new Error("Missing package input");
    files.push({ path: file.path, bytes: fs.statSync(file.source).size, sha256: await hash(file.source) });
  }
  if (!seen.has("deepagent.exe")) throw new Error("The packaged CLI is missing");
  fs.mkdirSync(output, { recursive: true });
  const setup = path.join(output, "setup.exe");
  fs.copyFileSync(nsis, setup);
  const manifest = { version, setup_sha256: await hash(setup), installed_bytes: files.reduce((n,f) => n + f.bytes, 0), files };
  fs.writeFileSync(path.join(output, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  return manifest;
}
module.exports = { hash, parseInstallFiles, stage };
if (require.main === module) {
  const [nsis, script, helper, output, version] = process.argv.slice(2);
  stage({ nsis, script, helper, output, version }).then(m => console.log(JSON.stringify({ version: m.version, files: m.files.length, bytes: m.installed_bytes })))
    .catch(e => { console.error(e); process.exitCode = 1; });
}
