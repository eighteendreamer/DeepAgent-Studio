import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

async function observe(explicitReconnect) {
  const root = await mkdtemp(path.join(os.tmpdir(), "deepagent-sdk-restart-"));
  const script = path.join(root, "strict-server.mjs");
  const log = path.join(root, "requests.jsonl");
  await writeFile(script, `
import { appendFileSync } from "node:fs";
import { createInterface } from "node:readline";
let initialized = false;
const log = process.argv[2];
createInterface({input: process.stdin}).on("line", line => {
  const request = JSON.parse(line);
  appendFileSync(log, JSON.stringify({pid:process.pid, method:request.method}) + "\\n");
  if (request.method === "audit/exit") process.exit(23);
  let response;
  if (request.method === "initialize") {
    initialized = true;
    response = {result:{protocolVersion:1, serverName:"strict-audit", serverVersion:"1", capabilities:{}}};
  } else if (!initialized) {
    response = {error:{code:-32001, message:"initialize must precede business requests"}};
  } else if (request.method === "thread/start") {
    response = {result:{threadId:"audit-thread", status:"ready"}};
  } else if (request.method === "thread/list") {
    response = {result:{threads:[]}};
  } else {
    response = {error:{code:-32601,message:"unknown method"}};
  }
  process.stdout.write(JSON.stringify({jsonrpc:"2.0",id:request.id,...response}) + "\\n");
});
`);
  const agent = new DeepAgent({command:process.execPath, args:[script,log]});
  try {
    assert.equal((await agent.startThread()).id, "audit-thread");
    // This rejection is delivered by the production exit callback, so a new
    // request starts only after it has cleared the dead child handle.
    await assert.rejects(agent.requestForTest("audit/exit", {}), HarnessTransportError);
    if (explicitReconnect) await agent.reconnect();
    let result;
    let failure;
    try { result = await agent.listThreads(); } catch (error) { failure = error; }
    const requests = (await readFile(log,"utf8")).trim().split("\n").map(JSON.parse);
    const groups = new Map();
    for (const request of requests) {
      if (!groups.has(request.pid)) groups.set(request.pid, []);
      groups.get(request.pid).push(request);
    }
    const generations = [...groups.values()].map(items => items.map(item => item.method));
    console.log(JSON.stringify({explicitReconnect, generations, result,
      error:failure && {name:failure.name,code:failure.code,message:failure.message}}));
    return {generations, result, failure};
  } finally {
    await agent.close();
    // SDK close does not await exit. Drop the fixture directory only after
    // its local request handlers have completed; no recursive user path.
    await rm(root, {recursive:true, force:true, maxRetries:10,retryDelay:25});
  }
}

test("explicit reconnect reinitializes a fresh stdio generation", {timeout:10000}, async () => {
  const value = await observe(true);
  assert.deepEqual(value.generations, [
    ["initialize","thread/start","audit/exit"], ["initialize","thread/list"]
  ]);
  assert.equal(value.failure, undefined);
  assert.deepEqual(value.result, []);
});

test("automatic stdio replacement must not reuse a dead generation's handshake", {
  skip: process.env.RUN_HARNESS_AUDIT !== "1", timeout:10000
}, async () => {
  const value = await observe(false);
  assert.equal(value.generations.length, 2, "fixture must observe a real new child");
  assert.equal(value.generations[1][0], "initialize",
    "new app-server received a business request without initialize");
  assert.equal(value.failure, undefined);
  assert.deepEqual(value.result, []);
});
