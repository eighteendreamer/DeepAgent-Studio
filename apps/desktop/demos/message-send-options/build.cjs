// Run from apps/desktop. Emits a standalone, offline HTML selection page.
const fs = require('node:fs');
const path = require('node:path');
const {pathToFileURL} = require('node:url');
const {createRequire} = require('node:module');
const desktop = path.resolve(__dirname,'../..');
const repo = path.resolve(desktop,'../..');
async function main(){
  const refined = process.argv.includes('--refined');
  const requireFromVite = createRequire(require.resolve('vite/package.json',{paths:[desktop]}));
  const esbuild = requireFromVite('esbuild');
  const result = esbuild.buildSync({entryPoints:[path.join(__dirname,refined?'refined.tsx':'main.tsx')],bundle:true,write:false,format:'esm',platform:'browser',jsx:'automatic',minify:true,define:{'process.env.NODE_ENV':'"production"'}});
  const {default:config} = await import(pathToFileURL(path.join(desktop,'postcss.config.js')).href);
  const postcss = require('postcss');
  const source = path.join(desktop,'src/styles.css');
  const extraCss = refined ? '\n'+fs.readFileSync(path.join(__dirname,'refined.css'),'utf8') : '';
  const css = (await postcss(config.plugins).process(fs.readFileSync(source,'utf8')+'\n'+fs.readFileSync(path.join(__dirname,'preview.css'),'utf8')+extraCss,{from:source})).css;
  const javascript = result.outputFiles[0].text.replace(/<\/script/gi,'<\\/script');
  const title = refined ? '消息发送 · F 轻 Q 弹' : '消息发送 · 六种连贯动效';
  const html = `<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${title}</title><style>${css}</style></head><body><div id="root"></div><script type="module">${javascript}</script></body></html>`;
  const output = path.join(repo,'.codex/previews',refined?'choice-preview-2026-10-22-0652-message-send-v2.html':'choice-preview-2026-10-22-0652-message-send.html');
  fs.mkdirSync(path.dirname(output),{recursive:true});
  fs.writeFileSync(output,html);
  console.log(output+' ('+Math.round(Buffer.byteLength(html)/1024)+' KiB)');
}
main().catch(error=>{console.error(error);process.exitCode=1;});
