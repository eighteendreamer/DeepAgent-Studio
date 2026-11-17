import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError, HarnessResultError } from "../dist/index.js";
const cases=JSON.parse(readFileSync(new URL("../../../crates/deepagent-harness-protocol/tests/fixtures/control_results_v1.json",import.meta.url),"utf8"));
const fixture=fileURLToPath(new URL("./result-fixture.mjs",import.meta.url));
function invalidResults(c){
  const values=[null,[],"PRIVATE_PAYLOAD",17,{}];
  for(const field of Object.keys(c.result)){
    const missing={...c.result};delete missing[field];values.push(missing);
    for(const value of [null,""," \t",{},[]])if(!(Array.isArray(value)&&Array.isArray(c.result[field])))values.push({...c.result,[field]:value});
  }
  const echo={"thread/resume":"threadId","thread/archive":"threadId","thread/unarchive":"threadId","thread/fork":"sourceThreadId",
    "turn/start":"threadId","turn/interrupt":"threadId","turn/steer":"threadId","approval/respond":"approvalId"}[c.method];
  if(echo)values.push({...c.result,[echo]:"PRIVATE_BAD_ID"});
  if(c.method==="turn/interrupt" || c.method==="turn/steer")values.push({...c.result,[c.method==="turn/steer"?"replacesTurnId":"turnId"]:"PRIVATE_BAD_ID"});
  if(c.method==="turn/steer")values.push({...c.result,turnId:c.params.turnId});
  if("status" in c.result)values.push({...c.result,status:"PRIVATE_BAD_STATUS"});
  if(c.method==="approval/respond")values.push({...c.result,approved:true});
  if(c.method==="thread/archive"||c.method==="thread/unarchive")values.push({...c.result,archived:c.method!=="thread/archive"});
  if(c.method==="event/ack")for(const ack of [6,8,-1,1.5])values.push({...c.result,acknowledgedEventSequence:ack});
  return values;
}
function peer(c,replies){return new DeepAgent({command:process.execPath,args:[fixture,JSON.stringify(replies),JSON.stringify(c.result)]});}

test("public fork and ACK wrappers keep receipt validation, RPC causes and local sequence precision", async () => {
  for (const method of ["thread/fork", "event/ack"]) {
    const c = cases.find(value => value.method === method);
    const agent = peer(c, [{}, {error:{code:-32602,message:"synthetic rejection"}}, c.result]);
    const call = () => method === "thread/fork"
      ? agent.forkThread(c.params.threadId, {atSequence:c.params.atSequence})
      : agent.acknowledgeEvents(c.params.eventSequence);
    try {
      await assert.rejects(call, HarnessResultError);
      await assert.rejects(call, error => error instanceof HarnessRpcError && error.code === -32602);
      assert.deepEqual(await call(), c.result);
      for (const sequence of [-1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
        await assert.rejects(() => method === "thread/fork"
          ? agent.forkThread(c.params.threadId, {atSequence:sequence})
          : agent.acknowledgeEvents(sequence), /safe unsigned integer/u);
      }
      assert.deepEqual(await agent.requestForTest("audit/count", {}), {calls:3});
    } finally { await agent.close(); }
  }
});
for(const c of cases){
  test(`H207 ${c.method} rejects invalid receipts without retry or value disclosure`,{timeout:10000},async()=>{
    const invalid=invalidResults(c),agent=peer(c,invalid);
    try{
      for(const _ of invalid){
        await assert.rejects(()=>agent.requestForTest(c.method,c.params),error=>{
          assert.ok(error instanceof HarnessResultError);assert.equal(error.method,c.method);
          assert.ok(error.field&&error.expected&&error.actual);assert.ok(!JSON.stringify(error).includes("PRIVATE_"));assert.ok(!error.message.includes("PRIVATE_"));return true;
        });
        assert.equal(agent.transport.pending.size,0);
      }
      assert.deepEqual(await agent.requestForTest("audit/count",{}),{calls:invalid.length});
    }finally{await agent.close();}
  });
  test(`H207 ${c.method} preserves valid extensions and original RPC rejection`,async()=>{
    const valid={...c.result,future:{opaque:"retained"}},agent=peer(c,[valid,{error:{code:-32602,message:"synthetic rejection"}}]);
    try{
      assert.deepEqual(await agent.requestForTest(c.method,c.params),valid);
      await assert.rejects(()=>agent.requestForTest(c.method,c.params),HarnessRpcError);
      assert.equal(agent.transport.pending.size,0);
      assert.deepEqual(await agent.requestForTest("audit/count",{}),{calls:2});
    }finally{await agent.close();}
  });
}
test("H207 public startThread cannot construct a thread from an invalid receipt",async()=>{
  const c=cases[0],agent=peer(c,[{}]);
  try{await assert.rejects(()=>agent.startThread());assert.deepEqual(await agent.requestForTest("audit/count",{}),{calls:1});}
  finally{await agent.close();}
});

test("H215 explicit archive list views reject an old or conflicting server projection",async()=>{
  for(const archived of [true,false]){
    const valid={threads:[{threadId:"thread-1",archived}]};
    const agent=peer({result:valid},[{threads:[{threadId:"thread-1"}]},{threads:[{threadId:"thread-1",archived:!archived}]},valid]);
    try{
      for(let index=0;index<2;index++)await assert.rejects(()=>agent.listThreads({archived}),HarnessResultError);
      assert.deepEqual(await agent.listThreads({archived}),valid.threads);
    }finally{await agent.close();}
  }
});
