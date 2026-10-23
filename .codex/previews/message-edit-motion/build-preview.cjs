const fs=require('node:fs');
const path=require('node:path');
const {createRequire}=require('node:module');
const {pathToFileURL}=require('node:url');
const root=path.resolve(__dirname,'../../..');
const desktop=path.join(root,'apps/desktop');
const req=createRequire(path.join(desktop,'package.json'));
const esbuild=createRequire(req.resolve('vite'))('esbuild');
async function build(){
  const filename=process.argv[2]||'choice-preview-2026-10-22-0825.html';
  if(!/^choice-preview-[\w-]+\.html$/.test(filename))throw Error('Use a versioned preview filename');
  const output=path.join(root,'.codex/previews',filename);
  if(fs.existsSync(output))throw Error('Preview exists. Choose another versioned filename.');
  const config=(await import(pathToFileURL(path.join(desktop,'tailwind.config.js')).href)).default;
  config.content=[path.join(desktop,'src/components/**/*.{ts,tsx}').replaceAll('\\','/'),path.join(__dirname,'preview.tsx').replaceAll('\\','/')];
  const corners=(await import(pathToFileURL(path.join(desktop,'scripts/continuous-corners.postcss.mjs')).href)).default;
  const source=fs.readFileSync(path.join(desktop,'src/styles.css'),'utf8')+'\n'+fs.readFileSync(path.join(__dirname,'preview.css'),'utf8');
  const css=await req('postcss')([req('tailwindcss')(config),corners(),req('autoprefixer')]).process(source,{from:path.join(desktop,'src/styles.css')});
  const bundle=await esbuild.build({entryPoints:[path.join(__dirname,'preview.tsx')],outfile:path.join(__dirname,'virtual.js'),absWorkingDir:desktop,nodePaths:[path.join(desktop,'node_modules')],bundle:true,write:false,format:'iife',minify:true,jsx:'automatic',target:'es2020',define:{'process.env.NODE_ENV':'"production"'},legalComments:'inline'});
  const js=bundle.outputFiles.find(file=>file.path.endsWith('.js')).text;
  const bundledCss=bundle.outputFiles.filter(file=>file.path.endsWith('.css')).map(file=>file.text).join('\n');
  const html=`<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>消息气泡编辑 · 六种动效选择 — DeepAgent</title><style>${css.css}\n${bundledCss}</style></head><body><div id="root"></div><script>${js.replace(/<\/script/gi,'<\\/script')}</script></body></html>`;
  fs.writeFileSync(output,html,'utf8');
  console.log(JSON.stringify({file:output,bytes:Buffer.byteLength(html),plans:6,productionChanged:false}));
}
build().catch(error=>{console.error(error);process.exitCode=1;});
