import assert from "node:assert/strict";
import test from "node:test";
import { mkdtempSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";
const fixture=fileURLToPath(new URL("./generation-fixture.mjs",import.meta.url));
function peer(mode="normal",options={}){
 const root=mkdtempSync(path.join(os.tmpdir(),"h265-generation-"));const log=path.join(root,"methods.jsonl");
 const agent=new DeepAgent({command:process.execPath,args:[fixture,mode,log],...options});
 const records=()=>{try{return readFileSync(log,"utf8").trim().split("\n").filter(Boolean).map(JSON.parse);}catch(error){if(error.code==="ENOENT")return [];throw error;}};
 return {agent,records,root};
}
const exit=(child)=>new Promise(resolve=>child.once("exit",resolve));
const methods=p=>p.records().filter(x=>x.method).map(x=>x.method);

test("H265 concurrent first calls share one physical generation handshake",{timeout:10000},async()=>{
 const p=peer();try{
  const results=await Promise.all(Array.from({length:12},()=>p.agent.listThreads()));
  assert.deepEqual(results,Array.from({length:12},()=>[]));
  assert.equal(p.agent.transport.initialized,true);
  assert.equal(methods(p).filter(x=>x==="initialize").length,1);
  assert.equal(new Set(p.records().map(x=>x.pid)).size,1);
 }finally{await p.agent.close();}
});

for(const mode of ["normal","exit-initialize","exit-admitted"]){
 test(`H265 ${mode} failure is terminal until explicit reconnect; no invisible spawn or mutation replay`,{timeout:10000},async()=>{
  const p=peer(mode);try{
   if(mode==="exit-initialize")await assert.rejects(()=>p.agent.startThread(),HarnessTransportError);
   else{
    const thread=await p.agent.startThread();
    const exited=exit(p.agent.transport.child);
    await assert.rejects(()=>mode==="exit-admitted"?thread.run("synthetic mutation"):p.agent.requestForTest("audit/exit",{}),HarnessTransportError);
    await exited;
   }
   const before=methods(p);
   const results=await Promise.allSettled(Array.from({length:12},()=>p.agent.listThreads()));
   assert.ok(results.every(x=>x.status==="rejected"&&x.reason instanceof HarnessTransportError));
   assert.equal(p.agent.transport.initialized,false);
   assert.deepEqual(methods(p),before);
   assert.equal(new Set(p.records().map(x=>x.pid)).size,1);
   assert.equal(p.agent.transport.pending.size,0);
   assert.equal(methods(p).filter(x=>x==="turn/start").length,mode==="exit-admitted"?1:0);
   if(mode!=="exit-initialize"){
    await p.agent.reconnect();await p.agent.resumeThread("thread-1");assert.deepEqual(await p.agent.listThreads(),[]);
    assert.equal(methods(p).filter(x=>x==="turn/start").length,mode==="exit-admitted"?1:0);
    const groups=new Map();for(const row of p.records().filter(x=>x.method)){if(!groups.has(row.pid))groups.set(row.pid,[]);groups.get(row.pid).push(row);}
    assert.equal(groups.size,2);for(const rows of groups.values())assert.equal(rows[0].method,"initialize");
   }
  }finally{await p.agent.close();}
 });
}

test("H265 explicit reconnects coalesce and old callbacks cannot invalidate a new ready generation",{timeout:10000},async()=>{
 const p=peer();try{
  await p.agent.startThread();const old=p.agent.transport;const child=old.child;
  // These are real installed callbacks, saved to simulate delayed delivery only
  // after the old process actually exits; never replace the production request.
  const oldExit=child.listeners("exit");const oldError=child.listeners("error");
  const oldStdoutClose=old.lines.listeners("close");const exited=exit(child);
  const callbacks=await Promise.all(Array.from({length:8},()=>p.agent.reconnect()));
  await exited;
  assert.equal(callbacks.length,8);
  assert.deepEqual(await p.agent.listThreads(),[]);
  const current=p.agent.transport;
  for(const callback of oldExit)callback(23,null);
  for(const callback of oldError)callback(new Error("synthetic late old error"));
  for(const callback of oldStdoutClose)callback();
  assert.equal(p.agent.transport,current);assert.equal(p.agent.transport.initialized,true);
  assert.deepEqual(await p.agent.listThreads(),[]);
  const groups=new Map();for(const row of p.records().filter(x=>x.method)){if(!groups.has(row.pid))groups.set(row.pid,[]);groups.get(row.pid).push(row);}
  assert.equal(groups.size,2);for(const rows of groups.values())assert.equal(rows[0].method,"initialize");
  assert.equal(current.pending.size,0);
 }finally{await p.agent.close();}
});

test("H265 close during a held handshake prevents stale readiness and settles all waiters",{timeout:10000},async()=>{
 const p=peer("hold-initialize");try{
  let ready;const held=new Promise(resolve=>ready=resolve);p.agent.subscribe(e=>{if(e.threadId==="initialize-held")ready();});
  const waiters=Array.from({length:6},()=>p.agent.startThread().then(value=>({value}),error=>({error})));
  await held;await p.agent.close();
  const outcomes=await Promise.all(waiters);assert.ok(outcomes.every(x=>x.error instanceof HarnessTransportError));
  assert.equal(p.agent.transport.initialized,false);assert.equal(p.agent.transport.pending.size,0);
  const before=methods(p);await assert.rejects(()=>p.agent.listThreads(),HarnessTransportError);assert.deepEqual(methods(p),before);
 }finally{await p.agent.close();}
});

test("H265 malformed stdout and stdin errors retire a generation without reusing its handshake",{timeout:10000},async()=>{
 for(const fault of ["malformed","stdin"]){
  const p=peer();try{
   await p.agent.startThread();
   if(fault==="malformed")await assert.rejects(()=>p.agent.requestForTest("audit/malformed",{}),HarnessTransportError);
   else p.agent.transport.child.stdin.emit("error",new Error("synthetic stdin failure"));
   const before=methods(p);await assert.rejects(()=>p.agent.listThreads(),HarnessTransportError);
   assert.equal(p.agent.transport.initialized,false);assert.equal(p.agent.transport.pending.size,0);assert.deepEqual(methods(p),before);
  }finally{await p.agent.close();}
 }
});

test("H265 an old approval handler must not respond on the replacement generation",{timeout:10000},async()=>{
 let release,asked;const answer=new Promise(resolve=>release=resolve);const requested=new Promise(resolve=>asked=resolve);
 const p=peer("normal",{onApproval:()=>{asked();return answer;}});
 try{
  await p.agent.startThread();await p.agent.requestForTest("audit/approval",{});await requested;
  await p.agent.reconnect();await p.agent.listThreads();release(true);await p.agent.waitForPendingApprovals();
  assert.equal(methods(p).filter(x=>x==="approval/respond").length,0,"old approval never crosses a generation boundary");
 }finally{release(false);await p.agent.close();}
});

test("H265 concurrent reconnect followed by close cannot resurrect a hidden child",{timeout:10000},async()=>{
 const p=peer();try{
  await p.agent.startThread();
  const transition=p.agent.reconnect().then(()=>({ok:true}),error=>({error}));
  await p.agent.close();await transition;
  const before=methods(p);await assert.rejects(()=>p.agent.listThreads(),HarnessTransportError);assert.deepEqual(methods(p),before);
 }finally{await p.agent.close();}
});

test("H265 spawn failure is shared, typed, and cannot initiate a hidden retry loop",{timeout:10000},async()=>{
 const p=peer();await p.agent.close();
 const missing=path.join(p.root,"does-not-exist.exe");
 const agent=new DeepAgent({command:missing});
 try{
  const results=await Promise.allSettled(Array.from({length:8},()=>agent.startThread()));
  assert.ok(results.every(x=>x.status==="rejected"&&x.reason instanceof HarnessTransportError));
  assert.equal(results[0].reason.cause.code,"ENOENT");
  assert.equal(agent.transport.nextId,2,"only one initialize attempt registered");
  assert.equal(agent.transport.pending.size,0);assert.equal(agent.transport.initialized,false);
  for(let index=0;index<3;index++)await assert.rejects(()=>agent.listThreads(),HarnessTransportError);
  assert.equal(agent.transport.nextId,2,"no automatic respawn or new request");
 }finally{await agent.close();}
});

for(const mode of ["normal","hold-initialize"]){
 test(`H265 actual stdout reader closure in ${mode} permanently fails the old generation`,{timeout:10000},async()=>{
  const p=peer(mode);try{
   let held;const barrier=new Promise(resolve=>held=resolve);p.agent.subscribe(e=>{if(e.threadId==="initialize-held")held();});
   const request=p.agent.startThread().then(value=>({value}),error=>({error}));
   if(mode==="normal")assert.ok((await request).value);else await barrier;
   const child=p.agent.transport.child;let observed;const failed=new Promise(resolve=>observed=resolve);
   const off=p.agent.transport.onFailure(observed);
   child.stdout.destroy();await failed;off();
   if(mode==="hold-initialize")assert.ok((await request).error instanceof HarnessTransportError);
   const before=methods(p);await assert.rejects(()=>p.agent.listThreads(),HarnessTransportError);
   assert.equal(p.agent.transport.initialized,false);assert.equal(p.agent.transport.pending.size,0);assert.deepEqual(methods(p),before);
  }finally{await p.agent.close();}
 });
}

test("H265 stdout-close failure before exit is already terminal and never writes an EPIPE retry",{timeout:10000},async()=>{
 const p=peer();try{
  await p.agent.startThread();const exited=exit(p.agent.transport.child);
  await assert.rejects(()=>p.agent.requestForTest("audit/exit",{}),HarnessTransportError);
  await assert.rejects(()=>p.agent.listThreads(),HarnessTransportError);
  await exited;assert.equal(new Set(p.records().map(x=>x.pid)).size,1);
  assert.deepEqual(methods(p),["initialize","thread/start","audit/exit"]);
 }finally{await p.agent.close();}
});

test("H265 explicit close before first call creates no child; only deliberate reconnect can reopen",{timeout:10000},async()=>{
 const p=peer();try{
  await p.agent.close();await p.agent.close();await assert.rejects(()=>p.agent.listThreads(),HarnessTransportError);
  assert.deepEqual(methods(p),[]);
  await p.agent.reconnect();assert.deepEqual(await p.agent.listThreads(),[]);
  assert.deepEqual(methods(p),["initialize","thread/list"]);
 }finally{await p.agent.close();}
});

test("H265 replacing a held handshake cannot authorize or complete requests on a newer generation",{timeout:10000},async()=>{
 const p=peer("hold-initialize");
 let signal;let barrier=new Promise(resolve=>signal=resolve);
 p.agent.subscribe(e=>{if(e.threadId==="initialize-held")signal();});
 try{
  const oldRequest=p.agent.startThread().then(value=>({value}),error=>({error}));await barrier;
  const old=p.agent.transport;await p.agent.reconnect();
  assert.ok((await oldRequest).error instanceof HarnessTransportError);
  barrier=new Promise(resolve=>signal=resolve);
  const currentRequest=p.agent.startThread();await barrier;
  const current=p.agent.transport;assert.notEqual(current,old);assert.equal(current.initialized,false);
  old.handleLine(JSON.stringify({jsonrpc:"2.0",id:1,result:{protocolVersion:1,serverName:"late-old",serverVersion:"1",capabilities:{threadLifecycle:true,streaming:true,interrupt:true,steer:true,approval:true,reconnect:true}}}));
  assert.equal(current.initialized,false);assert.equal(current.pending.size,1);
  await current.request("audit/release",{});assert.equal((await currentRequest).id,"thread-1");
  assert.equal(old.initialized,false);assert.equal(current.initialized,true);assert.equal(current.pending.size,0);
  assert.equal(methods(p).filter(x=>x==="initialize").length,2);
 }finally{await p.agent.close();}
});

test("H265 after close has completed a new deliberate reconnect must not reuse the superseded task",{timeout:10000},async()=>{
 const p=peer();try{
  await p.agent.startThread();
  const superseded=p.agent.reconnect().then(()=>({ok:true}),error=>({error}));
  await p.agent.close();
  await p.agent.reconnect();assert.deepEqual(await p.agent.listThreads(),[]);
  assert.ok((await superseded).error instanceof HarnessTransportError);
 }finally{await p.agent.close();}
});
