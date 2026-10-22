const fs=require('node:fs');
const path=require('node:path');
const project=path.resolve(__dirname,'../../..');
const desktop=path.join(project,'apps/desktop');
const ts=require(path.join(desktop,'node_modules/typescript'));
let field=fs.readFileSync(path.join(desktop,'src/components/ui/effortField.ts'),'utf8');
const signature='getRatio: () => number)';
const speed='(0.16 + ratio * 0.85)';
if(!field.includes(signature)||!field.includes(speed))throw new Error('Particle renderer changed: review the preview adapter.');
// Keep the existing particle art. Only this preview adds a drag-speed input.
field=field.replace(signature,'getRatio: () => number, getBoost: () => number = () => 1)').replace(speed,`${speed} * getBoost()`);
const compile=source=>ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2020,module:ts.ModuleKind.ESNext}}).outputText.replace(/export /g,'');
const clipboard=compile(fs.readFileSync(path.join(desktop,'src/clipboard.ts'),'utf8'));
const gsap=fs.readFileSync(path.join(desktop,'node_modules/gsap/dist/gsap.min.js'),'utf8');
const css=fs.readFileSync(path.join(__dirname,'choice.css'),'utf8');
const js=fs.readFileSync(path.join(__dirname,'choice.js'),'utf8');
const html=`<!doctype html><html lang="zh-CN"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>DeepAgent · 六种思考强度滑杆</title><style>${css}</style></head><body>
<main><header><p class="eyebrow">DEEPAGENT STUDIO / INTERACTION STUDIES</p><div class="heading"><div><h1>思考强度，选一种顺手的</h1><p>六种造型，都能点击、拖动。先感受滑行，再决定形状。</p></div><div class="toolbar"><button class="control-button" id="play-all" type="button">一起演示</button><button class="control-button" id="slow" type="button" aria-pressed="false">慢速观察</button><button class="control-button" id="theme" type="button" aria-pressed="false">深色预览</button></div></div></header>
<div class="note"><strong>点击有过程，拖动才跟手。</strong> 点击轨道或档位观察滑行；拖得越快，粒子流动越快，松手后缓下来。<span id="reduced-note" hidden> 当前系统启用了减少动态效果，动画已减弱。</span></div>
<section class="grid" aria-label="六种思考强度方案"></section><p class="footnote">A—E 为固定方案，F 可调宽高、滑行时长和拖动加速。此文件只用于预览选择，尚未替换正式控件。</p></main>
<footer class="selection-bar"><div class="selection-copy"><strong id="selection-title">先试手感，再选造型</strong><p id="selection-description">直接点击档位或拖动滑块；点卡片右上角确定选择</p><textarea readonly rows="2" class="choice-result" id="choice-result" aria-label="选择结果"></textarea></div><button class="copy-choice" id="copy-choice" disabled type="button">复制选择结果</button></footer><div class="toast" role="status" aria-live="polite"></div>
<script>${gsap.replace(/<\/script/gi,'<\\/script')}</script><script>${compile(field)}\n${clipboard}\n${js}</script></body></html>`;
const filename=process.argv[2]||'choice-preview-2026-10-22-0400.html';
if(!/^choice-preview-[\d-]+(?:-v\d+)?\.html$/.test(filename))throw new Error('Use a versioned preview filename.');
const output=path.join(project,'.codex/previews',filename);
if(fs.existsSync(output))throw new Error('Preview exists; create a versioned file instead of overwriting.');
fs.writeFileSync(output,html,'utf8');
console.log(JSON.stringify({file:output,bytes:Buffer.byteLength(html),plans:6}));
