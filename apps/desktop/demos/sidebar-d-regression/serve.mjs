// Browser-only QA server. Replaces the existing no-Tauri fallbacks in memory;
// never changes production API source, native settings, credentials or sessions.
import { createServer } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../',import.meta.url));
const now=Date.now();
const sessions=[
  {id:'layout-d',project:'DeepAgent Studio',title:'圆角与侧栏设计',mode:'normal',created_at:now,updated_at:now,ended:true,pinned:false},
  {id:'layout-docs',project:'DeepAgent Studio',title:'整理项目开发文档',mode:'normal',created_at:now-3600000,updated_at:now-3600000,ended:true,pinned:false},
  {id:'layout-recent',title:'工作台的深色主题',mode:'normal',created_at:now-7200000,updated_at:now-7200000,ended:true,pinned:false},
  {id:'layout-pinned',title:'常用提示词与设计约定',mode:'normal',created_at:now-14400000,updated_at:now-14400000,ended:true,pinned:true},
  {id:'layout-reference',project:'设计资料',title:'整理界面参考与灵感',mode:'normal',created_at:now-18000000,updated_at:now-18000000,ended:true,pinned:false},
  {id:'layout-tasks',title:'今天的开发计划',mode:'normal',created_at:now-21600000,updated_at:now-21600000,ended:true,pinned:false},
];
const conversation=[
  {role:'user',content:'参考这种侧栏和对话区的分隔方式，调整一下 DeepAgent 的布局。',parts:[]},
  {role:'assistant',content:'已经采用双栏导航。功能入口放在左侧窄栏，项目和会话集中在第二栏，对话区使用连续圆角面板。\n\n标题下方不再显示横线。',parts:[{kind:'text',text:'已经采用双栏导航。功能入口放在左侧窄栏，项目和会话集中在第二栏，对话区使用连续圆角面板。\n\n标题下方不再显示横线。'}]},
];
function fallback(code,name,from,to){
  const start=code.indexOf(`export async function ${name}(`),end=code.indexOf('\n}',start)+2;
  if(start<0||end<start||!code.slice(start,end).includes(from))throw Error(`QA fallback changed: ${name}`);
  return code.slice(0,start)+code.slice(start,end).replace(from,to)+code.slice(end);
}
const server=await createServer({
  root,configFile:false,plugins:[{name:'sidebar-layout-browser-fixtures',enforce:'pre',transform(code,id){
    if(!id.replaceAll('\\','/').endsWith('/src/api.ts'))return;
    code=fallback(code,'getSettings','return null;','return {configured:true,available_models:["deepseek-chat"],model_capabilities:[],chat_model:"deepseek-chat",reasoner_model:"deepseek-reasoner"};');
    code=fallback(code,'listSessions','return [];',`return ${JSON.stringify(sessions)};`);
    code=fallback(code,'listProjects','return [];',`return ${JSON.stringify([{name:'DeepAgent Studio',path:'F:/Preview/DeepAgent',pinned:false,session_count:2,updated_at:now},{name:'设计资料',path:'F:/Preview/Design',pinned:true,session_count:1,updated_at:now}])};`);
    code=fallback(code,'getSessionDetail','throw new Error("session detail requires the desktop app");',`return ${JSON.stringify({summary:sessions[0],timeline:[],conversation,stats:{event_count:2,messages:2,tool_calls:0,tool_successes:0,tool_failures:0,total_tool_ms:0,tool_success_rate:null,duration_ms:1000}})};`);
    return {code,map:null};
  }},react()],publicDir:false,cacheDir:'target/sidebar-d-20261022/vite-cache',
  server:{host:'127.0.0.1',port:5173,strictPort:true,watch:{ignored:['**/target/**','**/.tmp/**','**/public/file-viewer/**']}},optimizeDeps:{entries:['index.html']},
});
await server.listen();
console.log(`Browser QA (fixture data only): http://127.0.0.1:${server.httpServer.address().port}/#session=layout-d`);
