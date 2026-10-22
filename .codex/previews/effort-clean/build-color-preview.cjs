const fs=require('node:fs');
const path=require('node:path');
const project=path.resolve(__dirname,'../../..');
const folder=path.join(project,'.codex/previews');
const output=path.join(folder,'choice-preview-2026-10-22-0448-A-color.html');
if(fs.existsSync(output))throw Error('Use a new preview version.');
// Preserve the layout the user approved while other menu-shell work continues.
const approved=fs.readFileSync(path.join(folder,'choice-preview-2026-10-22-0438-A-clean.html'),'utf8');
const source=fs.readFileSync(path.join(project,'apps/desktop/src/styles.css'),'utf8');
const start=approved.indexOf('.reasoning-effort {');
const end=approved.indexOf('.reasoning-slider-track {',start);
if(start<0||end<start)throw Error('Approved preview CSS boundary missing.');
const css=source.slice(source.indexOf('.reasoning-effort {'),source.indexOf('.reasoning-slider-track {'));
fs.writeFileSync(output,approved.slice(0,start)+css+approved.slice(end),'utf8');
console.log(output);
