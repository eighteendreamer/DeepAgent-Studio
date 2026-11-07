// Explicit offline native fork readiness gate; no configured provider calls.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError, Thread } from "../dist/index.js";

test("H216 native cold forks are immediately addressable with durable scope; independent runs and restart do not duplicate branches", { timeout: 30000 }, async () => {
  const source = process.env.DEEPAGENT_TEST_BINARY ?? fileURLToPath(new URL(
    `../../../target/debug/deepagent${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  assert.ok(existsSync(source), "build the current CLI; no skipped fallback");
  const root = mkdtempSync(path.join(os.tmpdir(), "h216-real-cli-"));
  const binary = path.join(root, path.basename(source)); copyFileSync(source, binary);
  const hash = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
  assert.equal(hash(binary), hash(source));
  for (const key of Object.keys(process.env)) if (/API.?KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|PROXY|DEEPAGENT|DEEPSEEK|OPENAI|ANTHROPIC/iu.test(key)) delete process.env[key];
  for (const key of ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "DEEPAGENT_DATA_ROOT"]) {
    process.env[key] = path.join(root, key.toLowerCase()); mkdirSync(process.env[key]);
  }
  const host = path.join(root, "host"), secondHost = path.join(root, "second-host"), project = path.join(root, "project");
  for (const dir of [host, secondHost, project]) mkdirSync(dir);
  const clients = [], children = new Map();
  const open = async (cwd) => {
    const agent = new DeepAgent({ command: binary, cwd }); clients.push(agent);
    await agent.listThreads();
    const child = agent.transport.child;
    children.set(agent, { child, exited: new Promise((resolve) => child.once("exit", resolve)) });
    return agent;
  };
  const close = async (agent) => {
    await agent.close(); const owned = children.get(agent); if (!owned) return;
    await owned.exited; assert.throws(() => process.kill(owned.child.pid, 0), { code: "ESRCH" });
  };
  const rejected = (task) => assert.rejects(task, (error) => error instanceof HarnessRpcError && error.code === -32003);
  const read = (agent, id) => agent.readThreadPage(id, { maxBytes: 1048576 });
  const help = async (agent, id) => {
    const events = [];
    for await (const event of new Thread(agent, id).runStreamed("/help")) events.push(event);
    assert.equal(events.filter((event) => event.type === "turn.completed").length, 1);
  };
  try {
    const seed = await open(host);
    const parent = await seed.startThread({ cwd: project, permissionProfile: "read_only", sandboxBackend: "direct", reasoningEffort: "none" });
    const parentRow = (await seed.listThreads())[0];
    await close(seed);
    const cold = await open(secondHost);
    const before = await read(cold, parent.id); assert.equal(before.hasMore, false);
    assert.deepEqual(before.runEvents, []);
    const forks = await Promise.all([0, 1].map(() => cold.requestForTest("thread/fork", { threadId: parent.id, atSequence: 0 })));
    const ids = forks.map((fork) => fork.threadId);
    assert.equal(new Set([parent.id, ...ids]).size, 3);
    assert.equal((await cold.listThreads()).length, 3);
    for (const id of ids) {
      assert.equal((await cold.listThreads()).find((row) => row.threadId === id).cwd, parentRow.cwd);
      assert.deepEqual((await read(cold, id)).runEvents, []);
    }
    await rejected(() => cold.startTurn(parent.id, "/help")); // Fork never secretly loads the source.
    await Promise.all(ids.map((id) => help(cold, id))); // No resume of either child.
    assert.deepEqual(await read(cold, parent.id), before);
    for (const id of ids) {
      assert.deepEqual((await cold.resumeThread(id)).configuration, parent.configuration);
      const page = await read(cold, id); assert.equal(page.runEvents.length, 1);
      const run = page.runEvents[0]; assert.equal(run.terminalKind, "succeeded");
      assert.equal(run.events.filter((event) => event.event_type === "run_terminal").length, 1);
      assert.deepEqual(run.actions, []); assert.deepEqual(run.approvals, []);
    }
    await cold.resumeThread(parent.id);
    await help(cold, parent.id);
    const sourceAfter = await read(cold, parent.id);
    assert.equal(sourceAfter.runEvents.length, 1);
    for (const id of ids) assert.equal((await read(cold, id)).runEvents.length, 1);
    await close(cold);
    const owner = await open(secondHost); // Confirm source worker cleanup before archive.
    await owner.archiveThread(parent.id);
    await rejected(() => owner.requestForTest("thread/fork", { threadId: parent.id }));
    assert.equal((await owner.listThreads()).length + (await owner.listThreads({ archived: true })).length, 3);
    await owner.resumeThread(ids[0]);
    await help(owner, ids[0]); // A fork is independent, not a live subagent of the archived source.
    await close(owner);
    const reopened = await open(host);
    assert.equal((await reopened.listThreads()).length, 2);
    assert.deepEqual((await reopened.resumeThread(ids[0])).configuration, parent.configuration);
    assert.equal((await read(reopened, ids[0])).runEvents.length, 2);
    assert.equal((await read(reopened, ids[1])).runEvents.length, 1);
    assert.deepEqual(await read(reopened, parent.id), sourceAfter);
    console.log(JSON.stringify({ root, binarySha256: hash(binary), nativeProcesses: children.size, branches: 3, explicitRuns: 4 }));
  } finally { for (const agent of clients) await close(agent); }
});
