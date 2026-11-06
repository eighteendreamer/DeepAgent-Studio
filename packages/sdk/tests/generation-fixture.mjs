// Finite synthetic stdio generations. All records and IDs are test-only.
import { appendFileSync } from "node:fs";
import { createInterface } from "node:readline";
const [mode,log]=process.argv.slice(2);
const record=(value)=>appendFileSync(log,JSON.stringify({pid:process.pid,...value})+"\n");
const send=(id,result)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id,result})+"\n");
const error=(id,code,message)=>process.stdout.write(JSON.stringify({jsonrpc:"2.0",id,error:{code,message}})+"\n");
const ready={protocolVersion:1,serverName:"generation-peer",serverVersion:"1",capabilities:{threadLifecycle:true,streaming:true,interrupt:true,steer:true,approval:true,reconnect:true}};
let initialized=false,held;
process.on("exit",code=>record({exit:code}));
const input=createInterface({input:process.stdin});
input.on("close",()=>process.exit(0));
input.on("line",line=>{
 const r=JSON.parse(line);record({method:r.method,id:r.id});
 if(r.method==="initialize"){
  if(mode==="exit-initialize")process.exit(23);
  if(initialized){error(r.id,-32002,"initialize only once");return;}
  if(mode==="hold-initialize"){
   held=r;process.stdout.write(JSON.stringify({jsonrpc:"2.0",method:"harness/event",params:{type:"thread.started",threadId:"initialize-held",protocolVersion:1}})+"\n");return;
  }
  initialized=true;send(r.id,ready);return;
 }
 if(r.method==="audit/release"){
  if(held){initialized=true;send(held.id,ready);held=undefined;}
  send(r.id,{});return;
 }
 if(!initialized){error(r.id,-32001,"initialize first");return;}
 if(r.method==="audit/exit")process.exit(23);
 if(r.method==="audit/malformed"){process.stdout.write("{invalid json\n");return;}
 if(r.method==="thread/start"){send(r.id,{threadId:"thread-1",status:"ready"});return;}
 if(r.method==="thread/resume"){send(r.id,{threadId:r.params.threadId,status:"ready"});return;}
 if(r.method==="thread/list"){send(r.id,{threads:[]});return;}
 if(r.method==="turn/start"){
  if(mode==="exit-admitted"){
   record({admitted:r.id});process.exit(23);
  }
  send(r.id,{threadId:r.params.threadId,turnId:"turn-1",status:"started"});return;
 }
 if(r.method==="audit/approval"){
  process.stdout.write(JSON.stringify({jsonrpc:"2.0",method:"harness/event",params:{type:"approval.requested",approvalId:"old-approval",threadId:"thread-1",turnId:"turn-1",reason:"synthetic approval"}})+"\n");
  send(r.id,{});return;
 }
 if(r.method==="approval/respond"){send(r.id,{approvalId:r.params.approvalId,approved:r.params.approved,status:"resolved"});return;}
 error(r.id,-32601,"unknown synthetic method");
});
