const fs=require('node:fs');
const path=require('node:path');
const {createRequire}=require('node:module');
const {pathToFileURL}=require('node:url');
const project=path.resolve(__dirname,'../../..');
const desktop=path.join(project,'apps/desktop');
const desktopRequire=createRequire(path.join(desktop,'package.json'));
const esbuild=createRequire(desktopRequire.resolve('vite'))('esbuild');
async function build(){
 const name=process.argv[2];
 if(!name||!/^choice-preview-[\w-]+\.html$/.test(name))throw Error('Use a versioned HTML filename.');
 const output=path.join(project,'.codex/previews',name);if(fs.existsSync(output))throw Error('Preview exists; use a new version.');
 const config=(await import(pathToFileURL(path.join(desktop,'tailwind.config.js')).href)).default;
 config.content=[path.join(desktop,'src/**/*.{ts,tsx}').replaceAll('\\','/'),path.join(__dirname,'preview.tsx').replaceAll('\\','/')];
 const css=await desktopRequire('postcss')([desktopRequire('tailwindcss')(config),desktopRequire('autoprefixer')]).process(fs.readFileSync(path.join(desktop,'src/styles.css'),'utf8'),{from:path.join(desktop,'src/styles.css')});
 const bundled=await esbuild.build({entryPoints:[path.join(__dirname,'preview.tsx')],outfile:path.join(__dirname,'virtual.js'),absWorkingDir:desktop,nodePaths:[path.join(desktop,'node_modules')],bundle:true,write:false,format:'iife',minify:true,jsx:'automatic',target:'es2020',define:{'process.env.NODE_ENV':'"production"'},legalComments:'inline'});
 const importedCSS=bundled.outputFiles.filter(file=>file.path.endsWith('.css')).map(file=>file.text).join('\n');
 const html=`<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>A · 精简与对齐 — DeepAgent</title><style>${css.css}\n${importedCSS}\n${fs.readFileSync(path.join(__dirname,'preview.css'),'utf8')}</style></head><body><div id="root"></div><script>${bundled.outputFiles.find(file=>file.path.endsWith('.js')).text.replace(/<\/script/gi,'<\\/script')}</script></body></html>`;
 fs.writeFileSync(output,html,'utf8');console.log(JSON.stringify({file:output,bytes:Buffer.byteLength(html)}));
}
build().catch(error=>{console.error(error);process.exitCode=1;});
