// Explicit integration gate: build the real CLI first. No skipped fallback.
import assert from "node:assert/strict";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { DeepAgent } from "../dist/index.js";

test("real CLI early failure is delivered once and reconciles after reconnect with durable cursors", { timeout: 30000 }, async () => {
  const bin = fileURLToPath(new URL(`../../../target/debug/deepagent${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  assert.ok(existsSync(bin), "run cargo build -p deepagent-cli first");
  const root = mkdtempSync(path.join(os.tmpdir(), "deepagent-h214-real-cli-"));
  // Keep real executable bytes but no ambient bundled plugin/MCP preparation.
  const isolatedBin = path.join(root, path.basename(bin));
  copyFileSync(bin, isolatedBin);
  const hash = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
  assert.equal(hash(isolatedBin), hash(bin));
  const workspace = path.join(root, "workspace");
  // This file runs in its own node:test process. Only new synthetic roots are
  // exposed to the child; never seed a model setting or access OS credentials.
  for (const name of Object.keys(process.env)) {
    if (/(API.?KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|DEEPSEEK|OPENAI|ANTHROPIC)/iu.test(name)) delete process.env[name];
  }
  delete process.env.DEEPAGENT_DB_PATH;
  process.env.DEEPAGENT_DATA_ROOT = path.join(root, "data");
  for (const name of ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME"]) {
    process.env[name] = path.join(root, name.toLowerCase());
    mkdirSync(process.env[name], { recursive: true });
  }
  mkdirSync(workspace);
  const raw = [];
  const agent = new DeepAgent({ command: isolatedBin, cwd: workspace, onEvent: (event) => raw.push(event) });
  try {
    const thread = await agent.startThread({ permissionProfile: "read_only", sandboxBackend: "direct" });
    const events = [];
    for await (const event of thread.runStreamed("Synthetic local failure: no provider settings are installed.")) events.push(event);
    const terminals = events.filter((event) => /^turn\.(completed|failed|interrupted)$/u.test(event.type));
    assert.equal(terminals.length, 1);
    assert.equal(terminals[0].type, "turn.failed");
    assert.match(terminals[0].reason, /not initialized|API key/iu);
    const turnId = terminals[0].turnId;
    assert.ok(turnId);
    const capability = await agent.listTools({threadId: thread.id, runId: turnId});
    assert.ok(capability.snapshotId);
    assert.equal(capability.capabilitySnapshot.runId, turnId);
    assert.equal(capability.capabilitySnapshot.threadId, thread.id);
    assert.ok(capability.tools.some((tool) => tool.name === "read_file"));
    assert.ok(capability.tools.every((tool) => /^[a-f0-9]{64}$/u.test(tool.schemaHash)));
    assert.equal(raw.filter((event) => event.type === "turn.failed" && event.turnId === turnId).length, 1);
    const before = await agent.requestForTest("thread/read", { threadId: thread.id });
    assert.equal(before.runEvents.length, 1, "no automatic redispatch");
    const run = before.runEvents[0];
    assert.equal(run.runId, turnId);
    assert.equal(run.terminalKind, "failed");
    assert.equal(run.events.filter((event) => event.event_type === "run_terminal").length, 1);
    assert.deepEqual(run.actions, []);
    assert.deepEqual(run.approvals, []);
    await agent.reconnect();
    await agent.resumeThread(thread.id);
    const after = await agent.requestForTest("thread/read", { threadId: thread.id });
    assert.deepEqual(after, before, "the new connection reads the existing durable result, not a second run");
    assert.deepEqual(await agent.listTools({threadId: thread.id, runId: turnId,
      expectedSnapshotId: capability.snapshotId}), capability, "capabilities survive process restart without registry redispatch");
    await assert.rejects(() => agent.listTools({threadId: thread.id, expectedSnapshotId: "stale-snapshot"}),
      /stale capability snapshot/u);
    const cursor = run.events.at(-1).sequence;
    const incremental = await agent.requestForTest("thread/read", {
      threadId: thread.id, runAfterSequences: { [turnId]: cursor }
    });
    assert.deepEqual(incremental.runEvents[0].events, []);
    assert.equal(incremental.runEvents[0].terminalKind, "failed");
    // Controls have no provider/tool registry and must not need the unavailable
    // model backend or uninitialized credentials of this synthetic home.
    const control = await agent.startThread({sandboxBackend: "unavailable-model-backend"});
    await assert.rejects(() => control.run(`/resume ${thread.id}`), /cannot change its thread/u);
    const controlledEvents = [];
    for await (const event of control.runStreamed("/help")) controlledEvents.push(event);
    assert.equal(controlledEvents.filter((event) => event.type === "turn.completed").length, 1);
    const controlCapability = await control.listTools();
    assert.deepEqual(controlCapability.tools, []);
    assert.equal(controlCapability.capabilitySnapshot.context.executionDomain, "builtin_control");
  } finally { await agent.close(); }
});
