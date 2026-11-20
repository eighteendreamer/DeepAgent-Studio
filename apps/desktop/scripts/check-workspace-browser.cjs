// Native Windows acceptance. Only the loopback provider issues browser tool calls.
// Usage from the repository root: node apps/desktop/scripts/check-workspace-browser.cjs <desktop-exe>
// Set DEEPAGENT_TEST_PLAYWRIGHT to an installed Playwright module if it is not on Node's search path.
const { createServer }=require('node:http');
const { chromium }=require(process.env.DEEPAGENT_TEST_PLAYWRIGHT || 'playwright');
const { spawn }=require('node:child_process');
const fs=require('node:fs');
const path=require('node:path');
const assert=require('node:assert/strict');
const exec=process.argv[2];
assert(fs.existsSync(exec));
const root=path.resolve(process.env.DEEPAGENT_TEST_OUTPUT || 'target/right-browser-audit');
fs.mkdirSync(root, {recursive:true});
const data=fs.mkdtempSync(path.join(root,'native-data-'));
const port=Number(process.env.DEEPAGENT_TEST_CDP_PORT || 16472);
const toolName='workspace_browser';
const model='desktop-screenshot-fixture';
let image, count=0, providerError, armed=false, revoked=false, unauthorizedTurns=0;
const call=(action,args={})=>({type:'function_call',id:`fc_fixture${count}`,call_id:`call_fixture${count}`,name:toolName,arguments:JSON.stringify({action,...args}),status:'completed'});
const reply=(text)=>({type:'message',role:'assistant',id:'msg_fixture',status:'completed',content:[{type:'output_text',text,annotations:[]}]});
const evidence=[];

function completion(id,item){
 const added={...item,status:'in_progress',...(item.type==='function_call'?{arguments:''}:{content:[]})};
 const base={id,object:'response',created_at:1,model};
 const events=[{type:'response.created',response:{...base,status:'in_progress',output:[]}},{type:'response.output_item.added',output_index:0,item:added}];
 if(item.type==='function_call') events.push({type:'response.function_call_arguments.delta',output_index:0,item_id:item.id,delta:item.arguments},{type:'response.function_call_arguments.done',output_index:0,item_id:item.id,arguments:item.arguments});
 events.push({type:'response.output_item.done',output_index:0,item}, {type:'response.completed',response:{...base,status:'completed',output:[item],usage:{input_tokens:25,output_tokens:6,total_tokens:31},error:null,incomplete_details:null}});
 return events.map((e,sequence_number)=>`event: ${e.type}\ndata: ${JSON.stringify({...e,sequence_number})}\n\n`).join('');
}
const server=createServer(async(req,res)=>{
 try {
  if(req.url==='/favicon.ico'){res.statusCode=204; return res.end();}
  if(req.url==='/v1/models'){
   res.setHeader('Content-Type','application/json'); return res.end(JSON.stringify({object:'list',data:[{id:model,object:'model',input_modalities:['text','image']}]}));
  }
  if(['/fixture-start','/fixture-next'].includes(req.url)) {
    res.setHeader('Content-Type','text/html');
    return res.end(`<title>Right Browser Fixture</title><main style="background:#1f8bc5;color:white;font:28px Segoe UI;min-height:1200px"><h1>${req.url==='/fixture-start'?'AUDIT ENTRY':'AUDIT TARGET'}</h1><input aria-label="Test input" oninput="document.querySelector('output').textContent=event.target.value"><output></output><button onclick="document.querySelector('h1').textContent='AUDIT ACTION VERIFIED'">Apply test change</button></main>`);
  }
  if(req.url==='/files'||req.url==='/v1/files'){ req.resume(); res.statusCode=404; return res.end('{}'); }
  assert.equal(req.url,'/v1/responses');
  const chunks=[]; for await(const chunk of req) chunks.push(chunk);const body=JSON.parse(Buffer.concat(chunks));
  res.setHeader('Content-Type','text/event-stream');
  if (!armed) {
    if (body.tools?.some(tool=>tool.name==='screen_capture') || body.tools?.some(tool=>tool.name==='bash')) {
      assert(!body.tools?.some(tool=>tool.name===toolName), 'An ungranted or revoked tab must be excluded from the tool list');
      unauthorizedTurns++;
    }
    return res.end(completion('resp_seed',reply(revoked?'Fixture access revoked.':'Fixture session ready.')));
  }
  if (!body.tools?.some(tool=>tool.name===toolName) && !body.tools?.length) return res.end(completion('resp_title',reply('Browser fixture')));
  assert(body.tools?.some(tool=>tool.name===toolName), 'Native WorkspaceBrowser tool is missing');
  const prior=body.input.find(item=>item.type==='function_call_output'&&item.call_id===`call_fixture${count}`);
  const output=prior && (typeof prior.output==='string'?prior.output:prior.output.find(part=>part.type==='input_text')?.text);
  let result;
  if (output) {
    const parsed=JSON.parse(output);
    assert.notEqual(parsed.status,'error',output);
    result=parsed.result || parsed;
    evidence.at(-1).result=result;
  }
  let next;
  switch(++count) {
   case 1: next=call('navigate',{url:`http://127.0.0.1:${server.address().port}/fixture-next`}); break;
   case 2:
    assert.equal(result.status,'loaded');
    assert(result.url.endsWith('/fixture-next'));
    next=call('snapshot'); break;
   case 3: {
    assert.match(result.text,/AUDIT TARGET/);
    const target=result.elements.find(el=>el.label==='Test input').ref;
    next=call('fill',{target,text:'tool fixture value'}); break;
   }
   case 4: next=call('snapshot'); break;
   case 5: {
    assert.match(result.text,/tool fixture value/);
    const target=result.elements.find(el=>el.label==='Apply test change').ref;
    next=call('click',{target}); break;
   }
   case 6: next=call('scroll',{delta_y:180}); break;
   case 7: assert(result.scroll_y>0); next=call('scroll',{delta_y:-180}); break;
   case 8: next=call('snapshot'); break;
   case 9: assert.match(result.text,/AUDIT ACTION VERIFIED/); next=call('screenshot'); break;
   case 10: {
    assert(Array.isArray(prior.output),'Screenshot output must be structured');
    const parts=prior.output.filter(part=>part.type==='input_image');
    assert.equal(parts.length,1);
    assert.match(parts[0].image_url,/^data:image\/png;base64,/);
    image=Buffer.from(parts[0].image_url.split(',')[1],'base64');
    const width=image.readUInt32BE(16), height=image.readUInt32BE(20);
    assert(width>0&&width<800&&height>0&&height<1200, 'Screenshot must be the WebView viewport');
    evidence.at(-1).png={width,height,bytes:image.length};
    next=reply('Fixture model received right WebView screenshot.'); break;
   }
   default: throw Error('unexpected model request');
  }
  if(next.type==='function_call') evidence.push({tool:next.name,arguments:JSON.parse(next.arguments)});
  res.end(completion(`resp_fixture${count}`,next));
 } catch(error){providerError??=error; console.error('fixture response error:',String(error).slice(0,3500)); res.statusCode=400; res.end(JSON.stringify({error:{message:'fixture assertion failed'}}));}
});
const sleep=(ms)=>new Promise(r=>setTimeout(r,ms));
async function until(fn, ms=30000){const until=Date.now()+ms;while(Date.now()<until){try{const v=await fn(); if(v)return v;}catch{}await sleep(200);}throw Error('Timed out waiting for fixture');}
server.listen(0,'127.0.0.1',async()=>{
 let app,browser;
 try {
  app=spawn(exec,[],{cwd:path.dirname(exec),env:{...process.env,DEEPAGENT_DATA_ROOT:data,WEBVIEW2_USER_DATA_FOLDER:path.join(data,'webview'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`},stdio:['ignore','ignore','ignore']});
  await until(()=>fetch(`http://127.0.0.1:${port}/json/version`).then(res=>res.ok),60000);
  browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const page=await until(()=>browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('tauri.localhost/')&&!p.url().includes('splash')&&!p.url().includes('pet')),60000);
  await page.waitForFunction(()=>typeof window.__TAURI_INTERNALS__?.invoke==='function');
  const endpoint=`http://127.0.0.1:${server.address().port}/v1`;
  await page.evaluate(async({endpoint,model})=>{
   const call=(cmd,args)=>window.__TAURI_INTERNALS__.invoke(cmd,args);
   const p=await call('harness_provider_save',{provider:{channel:'Capture fixture',logo:'',modelUrl:endpoint,protocol:'responses',balanceKind:'none',enabled:true,apiKey:'fixture-key'}});
   const key={providerId:p.id,modelName:model};
   await call('harness_models_fetch',{providerId:p.id});
   await call('harness_model_set_active',{...key,active:true});
   await call('harness_model_select',key);
   await call('set_tool_search_mode',{mode:'disabled'});
   localStorage.setItem('onboarding_complete','1');
  },{endpoint,model});
  await page.reload();
  const editor=page.locator('.composer-textarea'); await editor.waitFor({timeout:60000});
  await editor.fill('Open a conversation for the right browser fixture.'); await editor.press('Enter');
  await page.locator('span.whitespace-pre-wrap').getByText('Fixture session ready.',{exact:true}).waitFor({timeout:60000});
  await page.getByRole('button',{name:'\u6253\u5f00\u53f3\u4fa7\u680f'}).click();
  await page.getByRole('button',{name:/^(?:Browser|\u6d4f\u89c8\u5668)(?:\s+Ctrl\+T)?$/i}).click();
  const addr=page.locator('input[placeholder="\u8f93\u5165 URL"]');
  const allow=page.getByRole('button',{name:'允许 AI 控制',exact:true});
  await allow.click();
  await page.evaluate(async()=>{window.__auditEvents=[];await window.__TAURI__.event.listen('chat://approval', e=>{const p=e.payload?.payload || e.payload;if(p.tool==='workspace_browser') void window.__TAURI_INTERNALS__.invoke('resolve_approval',{callId:p.call_id,approved:true,scope:'single_call'});});await window.__TAURI__.event.listen('chat://event', e=>{const p=e.payload?.payload || e.payload;if(['tool_started','tool_completed'].includes(p.type))window.__auditEvents.push(p)});});
  armed=true;
  await editor.fill('Use the existing right sidebar browser tab. Navigate, click, and take a screenshot.'); await editor.press('Enter');
  await page.getByText('Fixture model received right WebView screenshot.',{exact:true}).waitFor({timeout:45000});
  assert(image); assert.equal(count,10); assert.equal(providerError,undefined);
  const events=await page.evaluate(()=>window.__auditEvents);
  for(const call of evidence) {
    const start=events.find(e=>e.type==='tool_started'&&e.name===toolName&&Object.keys(call.arguments).every(key=>e.arguments?.[key]===call.arguments[key]));
    assert(start,`No start event for ${JSON.stringify(call.arguments)}`);
    const end=events.find(e=>e.type==='tool_completed'&&e.call_id===start.call_id);
    assert(end?.ok&&end.output,`No successful result event for ${start.call_id}`);
  }
  assert((await addr.inputValue()).endsWith('/fixture-next'),'Address bar must follow Harness navigation');
  async function checkDetails() {
    const turn=page.locator('[data-process-turn]').filter({hasText:'Fixture model received right WebView screenshot.'});
    const toggle=turn.locator('[data-process-toggle]');
    if (await toggle.getAttribute('aria-expanded') !== 'true') await toggle.click();
    const step=turn.getByRole('button',{name:/已查看 1 张图像|Viewed 1 image/});
    if (!(await step.isVisible())) {
      const group=turn.getByRole('button',{name:/\d+ 项操作|\d+ operations/});
      if(await group.isVisible()) await group.click();
    }
    await step.click();
    const detail=turn.locator('[data-browser-tool-details]').filter({hasText:'screenshot'});
    assert.match(await detail.getByLabel('浏览器请求参数').innerText(),/"action": "screenshot"/);
    assert.match(await detail.getByLabel('浏览器返回结果').innerText(),/"scope": "viewport"/);
    const thumb=turn.getByRole('button',{name:/预览图片|Preview image/i}).locator('img');
    await thumb.waitFor();
    const bytes=Buffer.from(await thumb.evaluate(async img=>Array.from(new Uint8Array(await (await fetch(img.src)).arrayBuffer()))));
    assert.deepEqual(bytes,image,'Provider and step preview must receive identical bytes');
  }
  await checkDetails();
  await page.getByRole('button',{name:'停止 AI 控制',exact:true}).click();
  await allow.waitFor();
  revoked=true; armed=false;
  await editor.fill('Verify browser access was revoked.'); await editor.press('Enter');
  await page.locator('span.whitespace-pre-wrap').getByText('Fixture access revoked.',{exact:true}).waitFor({timeout:20000});
  assert(unauthorizedTurns>=2);
  const sessionId=await page.locator('.sidebar-session-row[data-session-id]').first().getAttribute('data-session-id');
  await page.reload();
  await page.locator(`.sidebar-session-row[data-session-id="${sessionId}"]`).click();
  await page.locator('span.whitespace-pre-wrap').getByText('Fixture model received right WebView screenshot.',{exact:true}).waitFor({timeout:20000});
  await checkDetails();
  fs.writeFileSync(path.join(root,'native-evidence.json'),JSON.stringify({calls:evidence,permissionDeniedBeforeGrant:true,permissionRevoked:true,replayed:true},null,2));
  fs.writeFileSync(path.join(root,'native-viewport.png'),image);
  console.log('PASS: Harness navigated, filled, clicked, scrolled and captured the right WebView. Parameters, return values and identical PNG verified live and from replay.');
 } catch(error){console.error(providerError??error); console.error(JSON.stringify(evidence).slice(0,5000));process.exitCode=1;}
 finally{if(browser)await browser.close().catch(()=>{});app?.kill();server.close();}
});
