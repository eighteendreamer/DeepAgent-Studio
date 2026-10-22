import { build } from "vite";
import react from "@vitejs/plugin-react";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const desktop = fileURLToPath(new URL("../../", import.meta.url));
const output = resolve(desktop, "target/continuous-corners-preview/build");
await build({
  configFile: false,
  root: fileURLToPath(new URL(".", import.meta.url)),
  base: "./",
  plugins: [react()],
  build: { outDir: output, emptyOutDir: false, target: "es2020" },
});
let html = await readFile(resolve(output, "index.html"), "utf8");
const script = html.match(/<script\b[^>]*\bsrc="([^"]+)"[^>]*><\/script>/);
const stylesheet = html.match(/<link\b[^>]*\bhref="([^"]+\.css)"[^>]*>/);
if (!script || !stylesheet) throw new Error("Expected one bundled script and stylesheet");
const js = await readFile(resolve(output, script[1]), "utf8");
const css = await readFile(resolve(output, stylesheet[1]), "utf8");
html = html.replace(script[0], () => `<script type="module">${js.replace(/<\/script/gi, "<\\/script")}</script>`);
html = html.replace(stylesheet[0], () => `<style>${css}</style>`);
const standalone = resolve(output, "../continuous-corners.html");
await writeFile(standalone, html, "utf8");
console.log(`Standalone HTML: ${standalone}`);
