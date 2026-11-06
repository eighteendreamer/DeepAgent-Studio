// Explicit real-CLI gate. No provider, credentials, user data or network.
import assert from "node:assert/strict";
import { once } from "node:events";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { DeepAgent, HarnessRpcError, HarnessTransportError } from "../dist/index.js";

test("H265 real CLI failure stays failed; explicit fresh handshake/resume preserves one durable run",{timeout:30000},async()=>{
 const source=fileURLToPath(new URL(`../../../target/debug/deepagent${process.platform==="win32"?".exe":""}`,import.meta.url));
 assert.ok(existsSync(source),"build the current CLI first; no skipped fallback");
 const root=mkdtempSync(path.join(os.tmpdir(),"h265-real-cli-"));const binary=path.join(root,path.basename(source));
 copyFileSync(source,binary);const hash=p=>createHash("sha256").update(readFileSync(p)).digest("hex");assert.equal(hash(binary),hash(source));
 for(const key of Object.keys(process.env))if(/API.?KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|PROXY|DEEPAGENT|DEEPSEEK|OPENAI|ANTHROPIC/iu.test(key))delete process.env[key];
 for(const key of ["HOME","USERPROFILE","APPDATA","LOCALAPPDATA","XDG_CONFIG_HOME","DEEPAGENT_DATA_ROOT"]){process.env[key]=path.join(root,key.toLowerCase());mkdirSync(process.env[key]);}
 const workspace=path.join(root,"workspace");mkdirSync(workspace);
 const agent=new DeepAgent({command:binary,cwd:workspace});
 try{
  const thread=await agent.startThread({permissionProfile:"read_only",sandboxBackend:"direct",reasoningEffort:"none"});
  const events=[];for await(const event of thread.runStreamed("Synthetic local failure with no provider settings."))events.push(event);
  const terminals=events.filter(e=>/^turn\.(completed|failed|interrupted)$/u.test(e.type));assert.equal(terminals.length,1);assert.equal(terminals[0].type,"turn.failed");
  const before=await agent.readThreadPage(thread.id,{maxBytes:1048576});assert.equal(before.hasMore,false);assert.equal(before.runEvents.length,1);
  assert.equal(before.runEvents[0].terminalKind,"failed");assert.equal(before.runEvents[0].events.filter(e=>e.event_type==="run_terminal").length,1);
  assert.deepEqual(before.runEvents[0].actions,[]);assert.deepEqual(before.runEvents[0].approvals,[]);
  const previous=agent.transport,child=previous.child,pid=child.pid;const exited=once(child,"exit");assert.equal(child.kill(),true);await exited;
  const failures=await Promise.allSettled(Array.from({length:8},()=>agent.listThreads()));
  assert.ok(failures.every(x=>x.status==="rejected"&&x.reason instanceof HarnessTransportError));assert.equal(agent.transport,previous);assert.equal(previous.child,undefined);assert.equal(previous.pending.size,0);assert.equal(previous.initialized,false);
  await agent.reconnect();
  // A fresh server is not assumed to have loaded the old thread. No transparent
  // resume or replay is performed even though the database already contains it.
  await assert.rejects(()=>thread.run("must not be admitted without explicit resume"),e=>e instanceof HarnessRpcError&&e.code===-32003);
  const current=agent.transport;assert.notEqual(current.child.pid,pid);assert.equal(current.initialized,true);
  const [resumed,listed]=await Promise.all([agent.resumeThread(thread.id),agent.listThreads()]);assert.equal(listed.length,1);assert.deepEqual(resumed.configuration,thread.configuration);
  const after=await agent.readThreadPage(thread.id,{maxBytes:1048576});assert.equal(after.hasMore,false);
  assert.deepEqual(after.events,before.events);assert.deepEqual(after.runEvents,before.runEvents,"no hidden mutation retry or invented cancellation on process replacement");
  assert.equal(current.pending.size,0);assert.equal(hash(source),hash(binary));
 }finally{await agent.close();}
});
