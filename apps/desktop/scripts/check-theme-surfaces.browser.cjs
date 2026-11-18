// Real renderer components: CSS cascade, portal/morph surfaces and canvas isolation.
// Set THEME_TEST_PLAYWRIGHT to an external Playwright package when needed.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.THEME_TEST_PLAYWRIGHT || 'playwright');
const ts = require('typescript');
// Keep the external test runner off Vite/esbuild's application resolution path.
delete process.env.NODE_PATH;

const root = path.resolve(__dirname, '..');
const output = path.join(root, 'target/theme-surfaces');
const baseline = process.argv.includes('--baseline');
const referenceImage = process.env.THEME_TEST_WALLPAPER
  ? 'data:image/png;base64,' + fs.readFileSync(process.env.THEME_TEST_WALLPAPER).toString('base64') : null;
fs.mkdirSync(output, { recursive: true });
fs.writeFileSync(path.join(output, 'index.html'), '<!doctype html><html><head><meta charset="UTF-8"></head><body><div id="root"></div><script type="module" src="./main.tsx"></script></body></html>');
fs.writeFileSync(path.join(output, 'main.tsx'), `
import React, {useState} from 'react';
import {createRoot} from 'react-dom/client';
import '../../src/i18n'; import '../../src/icons'; import '../../src/styles.css';
import {bootstrapTheme} from '../../src/theme/themeBootstrap';
import {defaultV2State} from '../../src/theme/themeStorage';
import {ThemeProvider, useThemeContext} from '../../src/theme/ThemeProvider';
import {Composer} from '../../src/components/Composer';
import {SearchModal} from '../../src/components/SearchModal';
import {MarkdownText} from '../../src/components/MarkdownText';
import {UserMessageBubble} from '../../src/components/chat-timeline/MessageBubble';
import {AppNavigationRail} from '../../src/components/AppNavigationRail';
import {SettingsSidebar} from '../../src/components/SettingsSidebar';
import {SettingsView} from '../../src/components/SettingsView';
import {SkillsView} from '../../src/components/SkillsView';
import {KnowledgeView} from '../../src/components/KnowledgeView';
import {CanvasApp} from '../../src/canvas/CanvasApp';
import {Button} from '../../src/components/shadcn/button';
const params = new URLSearchParams(location.search);
const image = document.createElement('canvas'); image.width=800; image.height=500;
const ctx=image.getContext('2d');
for(let y=0;y<500;y+=40) for(let x=0;x<800;x+=40) {ctx.fillStyle=(x+y)%80 ? '#0b132b' : '#e8edff';ctx.fillRect(x,y,40,40);}
const state=defaultV2State(); state.mode=params.get('mode')||'light';
state.preferences.backgroundImage=params.get('wallpaper')==='off'?null:{dataUrl:${JSON.stringify(referenceImage)}||image.toDataURL('image/png'),name:${JSON.stringify(referenceImage ? 'reference-wallpaper.png' : 'contrast-checker.png')}};
state.preferences.backgroundTransparency=50;
const fixtureKey=location.search+location.hash;
if(sessionStorage.getItem('theme-fixture-key')!==fixtureKey) {
 localStorage.setItem('deepagent:theme:v2',JSON.stringify(state));
 sessionStorage.setItem('theme-fixture-key',fixtureKey);
}
localStorage.setItem('i18nextLng','zh');
bootstrapTheme();
const sessions=[{id:'fixture',title:'透明背景测试对话',project:'测试项目',updated_at:'2026-11-18T00:00:00Z'}];
const block={kind:'user',id:'fixture-user',text:'请检查这个用户消息气泡',attachments:[],selectedSkills:[],mentions:[],source:{id:'fixture-user',role:'user',content:'请检查这个用户消息气泡'}};
function Fixture(){
 const [view,setView]=useState('chat'),[category,setCategory]=useState('general'),[search,setSearch]=useState(false),[value,setValue]=useState('');
 const theme=useThemeContext();
 window.__themeFixture={update:theme.updatePreferences};
 if(location.hash==='#window=canvas') return <CanvasApp/>;
 return <div className="flex h-screen flex-col bg-sidebar-bg text-text-base">
 <div className="app-titlebar flex h-10 shrink-0 items-center gap-2 px-4"><span>背景表面验证</span>{['chat','settings','skills','knowledge'].map(v=><Button key={v} size="sm" variant="ghost" data-fixture-view={v} onClick={()=>setView(v)}>{v}</Button>)}</div>
 <div className="app-workspace"><AppNavigationRail activeSurface={view} sidebarOpen={true} onToggleSidebar={()=>{}} onHome={()=>setView('chat')} onNewChat={()=>setView('chat')} onOpenSearch={()=>setSearch(true)} onOpenSkills={()=>setView('skills')} onOpenKnowledge={()=>setView('knowledge')} onOpenPlugins={()=>{}} onOpenAutomation={()=>{}} onOpenRemote={()=>{}} onOpenCanvas={()=>{}} onOpenSettings={()=>setView('settings')} onLogout={()=>{}}/>
 <div className="app-content-frame">
 {view==='settings'&&<div className="app-sidebar-region"><SettingsSidebar activeCategoryId={category} onSelectCategory={setCategory} onBack={()=>setView('chat')}/></div>}
 <main className="app-main-surface relative min-w-0 flex-1 overflow-hidden bg-white">
 {view==='chat'&&<div data-surface="chat" className="view-frame"><div className="flex h-full flex-col gap-5 overflow-y-auto p-8">
 <Button data-fixture-search variant="outline" onClick={()=>setSearch(true)}>打开搜索</Button>
 <UserMessageBubble block={block} busy={false} onResend={()=>{}}/>
 <div data-fixture-output><MarkdownText text={'\\u0060\\u0060\\u0060text\\n/plan 进入只读 Plan 模式，先规划再执行\\n/plugin 查看插件管理入口\\n/settings 查看模型与权限设置\\n\\u0060\\u0060\\u0060'}/></div>
 <MarkdownText text={'\\u0060\\u0060\\u0060slash-panel\\n'+JSON.stringify({title:'斜杠命令',sections:[{heading:'可用命令',items:[{label:'/help',value:'查看命令帮助'}]}]})+'\\n\\u0060\\u0060\\u0060'}/>
 <div className="mt-auto"><Composer value={value} onChange={setValue} onSubmit={()=>{}} placeholder="尽管问…"/></div>
 </div></div>}
 {view==='settings'&&<div data-surface="settings" className="view-frame"><SettingsView activeCategoryId={category}/></div>}
 {view==='skills'&&<div data-surface="skills" className="view-frame"><SkillsView/></div>}
 {view==='knowledge'&&<div data-surface="knowledge" className="view-frame"><KnowledgeView/></div>}
 </main></div></div><SearchModal isOpen={search} onClose={()=>setSearch(false)} sessions={sessions} projects={[]} onSelectSession={()=>{}}/>
 </div>;
}
createRoot(document.getElementById('root')).render(<ThemeProvider><Fixture/></ThemeProvider>);
`);

async function run() {
  const { createServer } = await import('vite');
  const server = await createServer({ root, configFile: path.join(root, 'vite.config.ts'), logLevel: 'error',
    optimizeDeps: { entries: [path.join(output, 'index.html')] },
    server: { host: '127.0.0.1', port: 1432, strictPort: true, watch: null }, plugins: [{ name: 'theme-test-api', enforce: 'pre',
      resolveId(id, importer) { if (id === '@tauri-apps/api/webview' && importer?.replace(/\\/g, '/').endsWith('/src/canvas/CanvasApp.tsx')) return '\0theme-canvas-native'; },
      load(id) { if(id === '\0theme-canvas-native') return 'export function getCurrentWebview(){return {async onDragDropEvent(){return ()=>{};}}}'; },
      transform(source, id) {
        if (!id.replace(/\\/g, '/').endsWith('/src/api.ts')) return null;
        const ast=ts.createSourceFile(id,source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TS), edits=[];
        for(const node of ast.statements) if(ts.isFunctionDeclaration(node)&&node.body&&node.name?.text==='getSettings') edits.push({start:node.body.getStart(ast),end:node.body.end});
        for(const edit of edits.reverse()) source=source.slice(0,edit.start)+"{return {configured:true,available_models:['deepseek-flash'],model_capabilities:[],chat_model:'deepseek-flash',reasoning_effort:'high',sandbox_mode:'none'};}"+source.slice(edit.end);
        return source;
      },
    }] });
  await server.listen();
  const executablePath=[process.env['ProgramFiles(x86)'],process.env.ProgramFiles].filter(Boolean).map(dir=>path.join(dir,'Microsoft/Edge/Application/msedge.exe')).find(file=>fs.existsSync(file));
  const browser=await chromium.launch({headless:true,...(executablePath?{executablePath}:{})});
  const page=await browser.newPage({viewport:{width:1500,height:1000}}), errors=[], results=[];
  page.on('pageerror',error=>errors.push(error.stack || error.message));
  const url='http://127.0.0.1:1432/target/theme-surfaces/index.html';
  const sample=async(selector,label,kind='translucent')=>{
    const row=await page.locator(selector).first().evaluate((node,{label,kind})=>{
      const css=getComputedStyle(node), rgba=css.backgroundColor;
      const canvas=document.createElement('canvas');canvas.width=1;canvas.height=1;
      const context=canvas.getContext('2d');context.clearRect(0,0,1,1);context.fillStyle=rgba;context.fillRect(0,0,1,1);
      const alpha=context.getImageData(0,0,1,1).data[3]/255;
      return {label,kind,background:rgba,alpha,color:css.color,filter:css.backdropFilter};
    },{label,kind});
    results.push(row);
    if(!baseline) {
      assert.ok(kind==='opaque'?row.alpha===1:kind==='clear'?row.alpha===0:row.alpha>0&&row.alpha<1,JSON.stringify(row));
      assert.equal(row.filter,'none',JSON.stringify(row));
    }
    return row;
  };
  const contrast=async(selector,label,minimum=4.5)=>{
    const row=await page.locator(selector).first().evaluate((node,{label,minimum})=>{
      const canvas=document.createElement('canvas');canvas.width=1;canvas.height=1;const ctx=canvas.getContext('2d');
      const rgba=color=>{ctx.clearRect(0,0,1,1);ctx.fillStyle=color;ctx.fillRect(0,0,1,1);return Array.from(ctx.getImageData(0,0,1,1).data).map(value=>value/255);};
      let opacity=1;
      for(let parent=node;parent&&parent!==document.body;parent=parent.parentElement) opacity*=Number(getComputedStyle(parent).opacity);
      const luminance=rgb=>rgb.slice(0,3).map(value=>value<=.04045?value/12.92:((value+.055)/1.055)**2.4).reduce((sum,value,index)=>sum+value*[.2126,.7152,.0722][index],0);
      const fg=rgba(getComputedStyle(node).color);
      // Arbitrary images at 100% transparency have no fixed contrast. Check the
      // theme's foreground independently; screenshots cover the image itself.
      const bg=rgba(getComputedStyle(document.documentElement).getPropertyValue('--theme-bg'));
      const a=luminance(fg),b=luminance(bg);
      return {label,minimum,reference:'solid-theme-background',contrast:(Math.max(a,b)+.05)/(Math.min(a,b)+.05),opacity};
    },{label,minimum});
    results.push(row);
    if(!baseline) {assert.ok(row.contrast>=minimum,JSON.stringify(row));assert.ok(row.opacity>=.99,JSON.stringify(row));}
  };
  try {
    for(const mode of ['light','dark']) {
      await page.goto(url+'?mode='+mode,{waitUntil:'networkidle',timeout:120000});
      await page.locator('[data-message-composer-surface]').waitFor();
      await page.locator('[data-fixture-output] code').waitFor();
      await sample('[data-message-composer-surface]',mode+':composer');
      await contrast('.app-input-surface .text-gray-400',mode+':placeholder-contrast');
      await contrast('[data-nav="skills"] svg',mode+':navigation-icon-contrast',3);
      await sample('[data-user-message-id]',mode+':user-message');
      await sample('.markdown-body',mode+':system-output','clear');
      const codeBackground=await page.locator('[data-fixture-output] code').evaluate(node=>getComputedStyle(node).backgroundColor);
      if(!baseline) assert.equal(codeBackground,'rgba(0, 0, 0, 0)');
      await page.locator('.composer-textarea').fill('/');
      await page.locator('[data-composer-suggest]').waitFor();
      await page.waitForTimeout(450);
      await sample('[data-composer-suggest]',mode+':slash-suggestions');
      await contrast('[data-composer-suggest] .text-text-secondary',mode+':slash-label-contrast');
      await page.screenshot({path:path.join(output,(baseline?'before-':'after-')+mode+'-chat.png')});
      await page.locator('.composer-textarea').fill('');
      await page.locator('[data-fixture-search]').click();
      await page.getByPlaceholder('搜索对话').waitFor();
      await sample('.fixed .bg-elevated-bg',mode+':search-dialog');
      await contrast('.fixed .bg-elevated-bg .text-text-secondary',mode+':search-label-contrast');
      await page.keyboard.press('Escape');
      await page.locator('[data-nav="other"]').click();
      await page.locator('[data-nav-surface="plugins"]').waitFor();
      // SpringSurface paints the animated shell, whose background is copied from the menu.
      await page.waitForTimeout(650);
      await sample('[data-spring-shell]',mode+':other-menu');
      await contrast('[data-nav-surface="plugins"] svg',mode+':other-menu-icon-contrast',3);
      await page.keyboard.press('Escape');
      await page.locator('.app-navigation-bottom button[aria-label="设置"]').click();
      await page.locator('[data-menu-item="settings"]').waitFor();
      await page.waitForTimeout(650);
      await sample('[data-morph-panel]',mode+':settings-menu');
      await contrast('[data-menu-item="settings"] svg',mode+':settings-menu-icon-contrast',3);
      await page.keyboard.press('Escape');
      await page.locator('[data-fixture-view="settings"]').click();
      await page.locator('[data-settings-viewport] textarea').first().waitFor();
      await sample('[data-settings-viewport] textarea',mode+':settings-input');
      await sample('[data-settings-viewport] .bg-white',mode+':settings-card');
      await contrast('[data-settings-viewport] .bg-white .text-text-secondary',mode+':settings-description-contrast');
      await sample('[data-settings-viewport] .bg-white.rounded-full.absolute',mode+':switch-thumb','opaque');
      await page.screenshot({path:path.join(output,(baseline?'before-':'after-')+mode+'-settings.png')});
      await page.locator('[data-cat="appearance"]').click();
      const slider=page.getByRole('slider',{name:'背景透明度'});
      await slider.waitFor();
      // Exercise the real shadcn slider, including live preview, persistence,
      // both endpoints, and the user's reported 71% setting.
      for(const value of [0,71,100]) {
        await slider.focus();
        await page.keyboard.press('Home');
        if(value===100) await page.keyboard.press('End');
        else for(let step=0;step<value;step++) await page.keyboard.press('ArrowRight');
        await page.waitForTimeout(200);
        await sample('#root > .bg-sidebar-bg',mode+':window-chrome:'+value,value===0?'opaque':value===100?'clear':'translucent');
        await sample('[data-settings-category="appearance"] > section',mode+':appearance-card:'+value,value===0?'opaque':value===100?'clear':'translucent');
        await sample('.background-transparency-slider [role="slider"]',mode+':slider-thumb:'+value,'opaque');
        const state=await page.evaluate(()=>{
          const css=node=>getComputedStyle(node), frame=document.querySelector('.app-content-frame');
          const transparency=JSON.parse(localStorage.getItem('deepagent:theme:v2')).preferences.backgroundTransparency;
          return {transparency,imageOpacity:getComputedStyle(document.body,'::before').opacity,
            surfaceOpacity:css(document.documentElement).getPropertyValue('--app-surface-opacity').trim(),
            frameFill:css(frame).backgroundColor,railFill:css(document.querySelector('.app-navigation-rail')).backgroundColor,
            titleFill:css(document.querySelector('.app-titlebar')).backgroundColor,
            leftRadius:css(frame).borderTopLeftRadius,rightRadius:css(frame).borderTopRightRadius,
            clip:css(frame).overflow};
        });
        results.push({label:mode+':slider:'+value,...state});
        await page.screenshot({path:path.join(output,(baseline?'before-':'after-')+mode+'-appearance-'+value+'.png')});
        if(!baseline) {
          assert.equal(state.transparency,value);
          assert.equal(state.imageOpacity,'1');
          assert.ok(Math.abs(Number(state.surfaceOpacity)-(1-value/100))<1e-6);
          assert.equal(state.frameFill,'rgba(0, 0, 0, 0)');
          assert.equal(state.railFill,'rgba(0, 0, 0, 0)');
          assert.equal(state.titleFill,'rgba(0, 0, 0, 0)');
          assert.equal(state.leftRadius,state.rightRadius);
          assert.ok(parseFloat(state.leftRadius)>=24);
          assert.equal(state.clip,'hidden');
        }
      }
      await page.locator('[data-fixture-view="chat"]').click();
      await sample('[data-message-composer-surface]',mode+':fully-clear-composer','clear');
      await sample('[data-user-message-id]',mode+':fully-clear-message','clear');
      await page.locator('.composer-textarea').fill('/');
      await page.locator('[data-composer-suggest]').waitFor();
      await page.waitForTimeout(450);
      await sample('[data-composer-suggest]',mode+':fully-clear-slash','clear');
      await page.locator('.composer-textarea').fill('');
      await page.locator('[data-fixture-search]').click();
      await page.getByPlaceholder('搜索对话').waitFor();
      await sample('.fixed .bg-elevated-bg',mode+':fully-clear-search','clear');
      await page.keyboard.press('Escape');
      await page.locator('[data-nav="other"]').click();
      await page.locator('[data-nav-surface="plugins"]').waitFor();
      await page.waitForTimeout(650);
      await sample('[data-spring-shell]',mode+':fully-clear-other-menu','clear');
      await page.keyboard.press('Escape');
      await page.locator('.app-navigation-bottom button[aria-label="设置"]').click();
      await page.locator('[data-menu-item="settings"]').waitFor();
      await page.waitForTimeout(650);
      await sample('[data-morph-panel]',mode+':fully-clear-settings-menu','clear');
      await page.keyboard.press('Escape');
      await page.reload({waitUntil:'networkidle'});
      await page.locator('[data-message-composer-surface]').waitFor();
      await sample('[data-message-composer-surface]',mode+':reload-transparency','clear');
      await page.evaluate(()=>window.__themeFixture.update({backgroundTransparency:50}));
      await page.locator('[data-fixture-view="skills"]').click();
      await page.getByText('Code Review',{exact:true}).first().click();
      await page.locator('[data-surface="skills"] pre').first().waitFor();
      await sample('[data-surface="skills"] pre',mode+':skill-triggers');
      await sample('[data-surface="skills"] pre:last-of-type',mode+':skill-body');
      await page.screenshot({path:path.join(output,(baseline?'before-':'after-')+mode+'-skills.png')});
      await page.locator('[data-fixture-view="knowledge"]').click();
      await page.getByText('列表',{exact:true}).first().click();
      await page.getByText('PowerShell 管道命令被 ^C 中断',{exact:true}).first().click();
      await page.locator('[data-surface="knowledge"] pre').waitFor();
      await sample('[data-surface="knowledge"] pre',mode+':knowledge-body');
      await page.screenshot({path:path.join(output,(baseline?'before-':'after-')+mode+'-knowledge.png')});
    }
    await page.goto(url+'?mode=light#window=canvas',{waitUntil:'networkidle',timeout:120000});
    const canvasWallpaper=await page.locator('html').evaluate(node=>node.classList.contains('theme-custom-background'));
    results.push({label:'canvas-wallpaper',enabled:canvasWallpaper});
    if(!baseline) assert.equal(canvasWallpaper,false);
    await page.screenshot({path:path.join(output,'canvas.png')});
    await page.goto(url+'?wallpaper=off',{waitUntil:'networkidle',timeout:120000});
    await sample('[data-message-composer-surface]','no-wallpaper:composer','opaque');
    fs.writeFileSync(path.join(output,baseline?'before.json':'after.json'),JSON.stringify(results,null,2));
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({checks:results.length,errors,output,results},null,2));
  } catch(error) {
    fs.writeFileSync(path.join(output,'failure.html'),await page.content());
    fs.writeFileSync(path.join(output,'failure.json'),JSON.stringify({errors,results},null,2));
    await page.screenshot({path:path.join(output,'failure.png')});
    throw error;
  } finally { await browser.close(); await server.close(); }
}
run().catch(error=>{console.error(error);process.exitCode=1;});
