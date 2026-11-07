// Explicit offline native producer/consumer gate; no configured model or tools.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError } from "../dist/index.js";

test("H215 native archive gates stale hosts and cold resume; explicit restore preserves history without redispatch", { timeout: 30000 }, async () => {
  const source = process.env.DEEPAGENT_TEST_BINARY ?? fileURLToPath(new URL(
    `../../../target/debug/deepagent${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  assert.ok(existsSync(source), "build the current CLI; no skipped fallback");
  const root = mkdtempSync(path.join(os.tmpdir(), "h215-real-cli-"));
  const binary = path.join(root, path.basename(source));
  copyFileSync(source, binary);
  const hash = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
  assert.equal(hash(source), hash(binary));
  for (const key of Object.keys(process.env)) if (/API.?KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|PROXY|DEEPAGENT|DEEPSEEK|OPENAI|ANTHROPIC/iu.test(key)) delete process.env[key];
  for (const key of ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "DEEPAGENT_DATA_ROOT"]) {
    process.env[key] = path.join(root, key.toLowerCase()); mkdirSync(process.env[key]);
  }
  const workspace = path.join(root, "workspace"); mkdirSync(workspace);
  const clients = [], children = new Map();
  const open = async () => {
    const agent = new DeepAgent({ command: binary, cwd: workspace }); clients.push(agent);
    await agent.listThreads();
    const child = agent.transport.child;
    children.set(agent, { child, exited: new Promise((resolve) => child.once("exit", resolve)) });
    return agent;
  };
  const close = async (agent) => {
    await agent.close();
    const owned = children.get(agent); if (!owned) return;
    await owned.exited;
    assert.throws(() => process.kill(owned.child.pid, 0), { code: "ESRCH" });
  };
  const rejected = (task) => assert.rejects(task, (error) => error instanceof HarnessRpcError && error.code === -32003);
  const read = (agent, id) => agent.readThreadPage(id, { maxBytes: 1048576 });
  try {
    const first = await open();
    const original = await first.startThread({ permissionProfile: "read_only", sandboxBackend: "direct", reasoningEffort: "none" });
    const id = original.id;
    const initialEvents = [];
    for await (const event of original.runStreamed("/help")) initialEvents.push(event);
    assert.equal(initialEvents.filter((event) => event.type === "turn.completed").length, 1);
    await close(first); // Confirm worker cleanup before testing the idle contract.
    const owner = await open(), stale = await open();
    const loaded = await owner.resumeThread(id), oldView = await stale.resumeThread(id);
    const alias = `ses_${id.slice(4).toUpperCase()}`;
    await owner.resumeThread(alias);
    const before = await read(owner, id); assert.equal(before.hasMore, false); assert.equal(before.runEvents.length, 1);
    assert.deepEqual(await owner.archiveThread(id), { threadId: id, archived: true });
    assert.deepEqual(await owner.archiveThread(id), { threadId: id, archived: true });
    assert.deepEqual(await owner.listThreads(), []);
    assert.deepEqual((await owner.listThreads({ archived: true })).map((thread) => [thread.threadId, thread.archived]), [[id, true]]);
    await rejected(() => loaded.run("/help"));
    await rejected(() => owner.startTurn(alias, "/help"));
    await rejected(() => oldView.run("/help"));
    await rejected(() => stale.resumeThread(id));
    await rejected(() => owner.resumeThread(id));
    await rejected(() => owner.requestForTest("thread/fork", { threadId: id }));
    assert.deepEqual(await read(owner, id), before, "archive and denied mutations do not alter events, runs, actions or approvals");
    await close(stale); await close(owner);
    const cold = await open();
    assert.deepEqual(await cold.listThreads(), []);
    assert.equal((await cold.listThreads({ archived: true })).length, 1);
    await rejected(() => cold.resumeThread(id));
    assert.deepEqual(await cold.unarchiveThread(id), { threadId: id, archived: false });
    assert.deepEqual(await cold.unarchiveThread(id), { threadId: id, archived: false });
    assert.deepEqual(await cold.listThreads({ archived: true }), []);
    assert.equal((await cold.listThreads()).length, 1);
    await rejected(() => cold.startTurn(id, "/help")); // Restore does not secretly load a thread.
    const resumed = await cold.resumeThread(id);
    assert.deepEqual(resumed.configuration, original.configuration);
    assert.deepEqual(await read(cold, id), before);
    const finalEvents = [];
    for await (const event of resumed.runStreamed("/help")) finalEvents.push(event);
    assert.equal(finalEvents.filter((event) => event.type === "turn.completed").length, 1);
    const after = await read(cold, id); assert.equal(after.runEvents.length, 2);
    assert.deepEqual(after.runEvents.find((run) => run.runId === before.runEvents[0].runId), before.runEvents[0]);
    for (const run of after.runEvents) {
      assert.equal(run.terminalKind, "succeeded");
      assert.equal(run.events.filter((event) => event.event_type === "run_terminal").length, 1);
      assert.deepEqual(run.actions, []); assert.deepEqual(run.approvals, []);
    }
    console.log(JSON.stringify({ root, binarySha256: hash(binary), nativeProcesses: children.size, durableRuns: after.runEvents.length }));
  } finally { for (const agent of clients) await close(agent); }
});
