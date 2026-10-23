const fs=require('node:fs');
const path=require('node:path');
const {createRequire}=require('node:module');
const project=path.resolve(__dirname,'../../..');
const desktop=path.join(project,'apps/desktop');
const desktopRequire=createRequire(path.join(desktop,'package.json'));
const esbuild=createRequire(desktopRequire.resolve('vite'))('esbuild');
async function build(){
 const bundled=await esbuild.build({entryPoints:[path.join(__dirname,'preview.tsx')],absWorkingDir:desktop,nodePaths:[path.join(desktop,'node_modules')],bundle:true,write:false,format:'iife',minify:true,jsx:'automatic',target:'es2020',define:{'process.env.NODE_ENV':'"production"'},legalComments:'inline'});
 const source=fs.readFileSync(path.join(desktop,'src/styles.css'),'utf8');
 const css=source.slice(source.indexOf('.reasoning-effort {'),source.indexOf('.reasoning-slider-track {'))+'\n'+fs.readFileSync(path.join(__dirname,'preview.css'),'utf8');
 const html=`<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>A · 果冻圆珠 — DeepAgent</title><style>${css}</style></head><body><div id="root"></div><script>${bundled.outputFiles[0].text.replace(/<\/script/gi,'<\\/script')}</script></body></html>`;
 const name=process.argv[2]||'choice-preview-2026-10-22-0416-A-jelly.html';
 if(!/^choice-preview-[\w-]+\.html$/.test(name))throw Error('Use a versioned HTML filename.');
 const output=path.join(project,'.codex/previews',name);if(fs.existsSync(output))throw Error('Preview exists; use a new version.');
 fs.writeFileSync(output,html,'utf8');console.log(JSON.stringify({file:output,bytes:Buffer.byteLength(html)}));
}
build().catch(error=>{console.error(error);process.exitCode=1;});
