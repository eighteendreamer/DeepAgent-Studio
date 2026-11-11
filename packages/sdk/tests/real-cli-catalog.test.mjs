// Explicit native protocol gate; isolated homes, synthetic metadata, no models.
import assert from "node:assert/strict";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError } from "../dist/index.js";

test("H220 real CLI catalog pages survive cold reopen and fail explicitly on mutation",{timeout:60000},async()=>{
  const source=process.env.DEEPAGENT_TEST_BINARY ?? fileURLToPath(new URL(
    `../../../target/debug/deepagent${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  assert.ok(source && existsSync(source),"provide the freshly built isolated CLI, never skip");
  const root=mkdtempSync(path.join(os.tmpdir(),"h220-native-"));
  const binary=path.join(root,path.basename(source));copyFileSync(source,binary);
  for(const key of Object.keys(process.env))if(/API.?KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|PROXY|DEEPAGENT|DEEPSEEK|OPENAI|ANTHROPIC/iu.test(key))delete process.env[key];
  for(const key of ["HOME","USERPROFILE","APPDATA","LOCALAPPDATA","XDG_CONFIG_HOME","DEEPAGENT_DATA_ROOT"]){process.env[key]=path.join(root,key.toLowerCase());mkdirSync(process.env[key]);}
  const workspace=path.join(root,"workspace");mkdirSync(workspace);
  const clients=[];
  const open=()=>{const agent=new DeepAgent({command:binary,cwd:workspace});clients.push(agent);return agent;};
  try {
    const first=open();
    const ids=[];
    for(let n=0;n<58;n++)ids.push((await first.startThread({provider:n%2?"openai":"deepseek",permissionProfile:"read_only"})).id);
    assert.equal((await first.listThreads()).length,25,"legacy convenience is first page only");
    const page=await first.listThreadsPage();assert.equal(page.threads.length,25);assert.equal(page.hasMore,true);
    await first.close();
    const cold=open();
    const seen=new Set(page.threads.map(row=>row.threadId));
    let next=page.nextCursor;
    while(next){const result=await cold.listThreadsPage({cursor:next});for(const row of result.threads){assert.ok(!seen.has(row.threadId));seen.add(row.threadId);}next=result.nextCursor;}
    assert.deepEqual([...seen].sort(),ids.sort());
    const provider=await cold.listThreadsPage({provider:"deepseek",limit:100});assert.equal(provider.threads.length,29);assert.equal(provider.hasMore,false);
    const stale=await cold.listThreadsPage({limit:1});
    await cold.archiveThread(ids[0]);
    await assert.rejects(()=>cold.listThreadsPage({cursor:stale.nextCursor}),error=>error instanceof HarnessRpcError && error.code===-32602 && /stale/u.test(error.message));
    assert.equal((await cold.listThreadsPage({archived:true})).threads.length,1);
    await cold.unarchiveThread(ids[0]);
    assert.equal((await cold.listThreadsPage({archived:true})).threads.length,0);
    console.log(JSON.stringify({root,pages:3,uniqueSessions:seen.size,explicitProviderMatches:provider.threads.length}));
  }finally{for(const client of clients)await client.close();}
});
