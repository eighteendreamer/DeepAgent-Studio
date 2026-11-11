import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { DeepAgent } from "../dist/index.js";

const fixture=fileURLToPath(new URL("result-fixture.mjs",import.meta.url));
const row={threadId:"synthetic",title:null,cwd:null,createdAt:1,updatedAt:2,ended:false,archived:false};
const page={listPageVersion:1,threads:[row],hasMore:true,nextCursor:"next"};

test("H220 explicit catalog reads one page, including empty advancing pages, without an implicit loop",async()=>{
  const second={...page,threads:[],nextCursor:"last"};
  const done={...page,threads:[],hasMore:false,nextCursor:null};
  const agent=new DeepAgent({command:process.execPath,args:[fixture,JSON.stringify([page,second,done]),"null"]});
  try {
    assert.deepEqual(await agent.listThreadsPage({limit:1}),page);
    assert.equal((await agent.requestForTest("audit/count",{})).calls,1);
    assert.deepEqual(await agent.listThreadsPage({cursor:"next"}),second);
    assert.deepEqual(await agent.listThreadsPage({cursor:"last"}),done);
    assert.equal((await agent.requestForTest("audit/count",{})).calls,3);
  }finally{await agent.close();}
});

test("H220 explicit catalog reader rejects legacy, duplicate, over-limit and looping peer results",async()=>{
  const bad=[{threads:[]},{...page,listPageVersion:2},{...page,threads:[row,row]},
    {...page,nextCursor:"previous"},{...page,hasMore:false},{...page,nextCursor:null},
    {...page,threads:[{...row,updatedAt:1.5}]},{...page,threads:[{...row,archived:"false"}]}];
  const agent=new DeepAgent({command:process.execPath,args:[fixture,JSON.stringify(bad),"null"]});
  try {
    for(const _ of bad) await assert.rejects(()=>agent.listThreadsPage({limit:1,cursor:"previous"}),/bounded catalog page/u);
    assert.equal((await agent.requestForTest("audit/count",{})).calls,bad.length);
  }finally{await agent.close();}
});
