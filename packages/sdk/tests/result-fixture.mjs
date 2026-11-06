import { createInterface } from "node:readline";
const replies=JSON.parse(process.argv[2]);
const valid=JSON.parse(process.argv[3]);
let calls=0;
const send=(value)=>process.stdout.write(JSON.stringify(value)+"\n");
createInterface({input:process.stdin}).on("line",line=>{
  const r=JSON.parse(line);
  if(r.method==="initialize"){
    send({jsonrpc:"2.0",id:r.id,result:{protocolVersion:1,serverName:"receipt-peer",serverVersion:"1",
      capabilities:{threadLifecycle:true,streaming:true,interrupt:true,steer:true,approval:true,reconnect:true}}});
  }else if(r.method==="audit/count")send({jsonrpc:"2.0",id:r.id,result:{calls}});
  else{
    const reply=replies[calls++];
    send({jsonrpc:"2.0",id:r.id,...(reply?.error?reply:{result:reply})});
    send({jsonrpc:"2.0",id:r.id,result:valid});
    send({jsonrpc:"2.0",method:"harness/event",params:{type:"thread.started",threadId:"receipt-barrier",protocolVersion:1}});
  }
});
