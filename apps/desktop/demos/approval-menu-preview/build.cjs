// Run from apps/desktop: node demos/approval-menu-preview/build.cjs
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");
const { pathToFileURL } = require("node:url");
const desktop = path.resolve(__dirname, "../..");
const viteRequire = createRequire(require.resolve("vite/package.json", { paths: [desktop] }));
const esbuild = viteRequire("esbuild");
async function main() {
  const result = esbuild.buildSync({
    entryPoints: [path.join(__dirname, "main.tsx")], outfile: path.join(__dirname, "virtual.js"), bundle: true, write: false,
    format: "esm", platform: "browser", jsx: "automatic", minify: true,
    define: { "process.env.NODE_ENV": '"production"' },
  });
  const postcss = require("postcss");
  const { default: config } = await import(pathToFileURL(path.join(desktop, "postcss.config.js")).href);
  const cssSource = path.join(desktop, "src/styles.css");
  const baseCSS = fs.readFileSync(cssSource, "utf8");
  const previewCSS = fs.readFileSync(path.join(__dirname, "preview.css"), "utf8");
  // Preview styles must use the same corner conversion as the desktop UI.
  const stylesheet = (await postcss(config.plugins).process(`${baseCSS}\n${previewCSS}`, { from: cssSource })).css;
  const javascript = result.outputFiles.find(file => file.path.endsWith(".js")).text.replace(/<\/script/gi, "<\\/script");
  const importedCSS = result.outputFiles.filter(file => file.path.endsWith(".css")).map(file => file.text).join("\n");
  const html = `<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>审批菜单 · 交互预览</title><style>${stylesheet}\n${importedCSS}</style></head><body><div id="root"></div><script type="module">${javascript}</script></body></html>`;
  fs.writeFileSync(path.join(__dirname, "index.html"), html);
  console.log(`Standalone preview: ${path.join(__dirname, "index.html")} (${Math.round(Buffer.byteLength(html) / 1024)} KiB)`);
}
main().catch(error => { console.error(error); process.exitCode = 1; });
