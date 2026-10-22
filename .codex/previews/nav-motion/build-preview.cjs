const fs=require('node:fs');
const path=require('node:path');
const {createRequire}=require('node:module');
const {pathToFileURL}=require('node:url');
const project=path.resolve(__dirname,'../../..');
const desktop=path.join(project,'apps/desktop');
const req=createRequire(path.join(desktop,'package.json'));
const esbuild=createRequire(req.resolve('vite'))('esbuild');
async function build(){
 const name=process.argv[2];
 if(!name||!/^choice-preview-[\w-]+\.html$/.test(name))throw Error('Use a versioned filename');
 const output=path.join(project,'.codex/previews',name);
 if(fs.existsSync(output))throw Error('Preview exists; use a new version');
 const applied=process.argv[3]==='applied';
 const entry=applied?'applied':'preview';
 const config=(await import(pathToFileURL(path.join(desktop,'tailwind.config.js')).href)).default;
 config.content=[path.join(desktop,'src/components/**/*.{ts,tsx}').replaceAll('\\','/'),path.join(__dirname,entry+'.tsx').replaceAll('\\','/')];
 const corners=(await import(pathToFileURL(path.join(desktop,'scripts/continuous-corners.postcss.mjs')).href)).default;
 const source=applied?fs.readFileSync(path.join(desktop,'src/styles.css'),'utf8'):'@tailwind base;\n@tailwind utilities;';
 const css=await req('postcss')([req('tailwindcss')(config),corners(),req('autoprefixer')]).process(source,{from:applied?path.join(desktop,'src/styles.css'):undefined});
 const bundle=await esbuild.build({entryPoints:[path.join(__dirname,entry+'.tsx')],outfile:path.join(__dirname,'virtual-preview.js'),absWorkingDir:desktop,nodePaths:[path.join(desktop,'node_modules')],bundle:true,write:false,format:'iife',minify:true,jsx:'automatic',target:'es2020',define:{'process.env.NODE_ENV':'"production"'},legalComments:'inline'});
 const bundledCss=bundle.outputFiles.filter(file=>file.path.endsWith('.css')).map(file=>file.text).join('\n');
 const js=bundle.outputFiles.find(file=>file.path.endsWith('.js')).text;
 const html=`<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${applied?'E · 轻弹即停接入效果':'导航高亮动效 · 六种手感'} — DeepAgent</title><style>${css.css}\n${bundledCss}\n${fs.readFileSync(path.join(__dirname,entry+'.css'),'utf8')}</style></head><body><div id="root"></div><script>${js.replace(/<\/script/gi,'<\\/script')}</script></body></html>`;
 fs.writeFileSync(output,html,'utf8');console.log(JSON.stringify({file:output,bytes:Buffer.byteLength(html)}));
}
build().catch(error=>{console.error(error);process.exitCode=1;});
