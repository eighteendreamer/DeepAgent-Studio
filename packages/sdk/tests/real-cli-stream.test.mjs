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
  const bin = process.env.DEEPAGENT_TEST_BINARY ?? fileURLToPath(new URL(`../../../target/debug/deepagent${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
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
    const thread = await agent.startThread({ permissionProfile: "read_only", sandboxBackend: "direct", reasoningEffort: "none" });
    assert.equal(thread.configuration.overrides.reasoning_effort, "none");
    const config = await agent.readConfig({threadId: thread.id});
    assert.equal(config.configSnapshotVersion, 1);
    assert.equal(config.resolutionStatus, "uninitialized");
    assert.equal(config.credentialChecked, false);
    assert.equal(config.provider, null);
    assert.equal(config.protocol, null);
    assert.equal(config.providerRevision, null);
    assert.equal(config.modelConfigHash, null);
    assert.equal(config.permissionProfile.sandboxMode, "read_only");
    assert.equal(config.sandboxBackend, "direct");
    await assert.rejects(() => thread.run("reject stale config", {expectedConfigSnapshotId: "stale-config"}), /stale execution configuration/u);
    const events = [];
    for await (const event of thread.runStreamed("Synthetic local failure: no provider settings are installed.")) events.push(event);
    const terminals = events.filter((event) => /^turn\.(completed|failed|interrupted)$/u.test(event.type));
    assert.equal(terminals.length, 1);
    assert.equal(terminals[0].type, "turn.failed");
    assert.match(terminals[0].reason, /not initialized|API key/iu);
    const turnId = terminals[0].turnId;
    assert.ok(turnId);
    const capability = await agent.listTools({threadId: thread.id, runId: turnId});
    assert.equal(capability.capabilitySnapshot.context.executionConfig.configSnapshotId, config.configSnapshotId);
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
    // Exercise the public bounded API over real stdio, including a process
    // restart in the middle. Compare actual rows, not random token inequality.
    let history = await agent.readThreadPage(thread.id, {pageSize: 5, maxBytes: 1048576});
    const collected = {events: [], runEvents: [], actions: [], approvals: [], artifacts: [], nodes: []};
    let pages = 0;
    for (; pages < 100; pages++) {
      let items = history.events.length;
      collected.events.push(...history.events);
      for (const slice of history.runEvents) {
        assert.equal(slice.runId, turnId);
        items += 1 + slice.events.length + slice.actions.length + slice.approvals.length
          + slice.artifacts.length + slice.graph.nodes.length;
        collected.runEvents.push(...slice.events);
        for (const field of ["actions", "approvals", "artifacts"]) collected[field].push(...slice[field]);
        collected.nodes.push(...slice.graph.nodes);
      }
      assert.ok(items <= 5);
      if (!history.hasMore) break;
      if (pages === 0) await agent.reconnect();
      history = await agent.readThreadPage(thread.id, {cursor: history.nextCursor, pageSize: 5, maxBytes: 1048576});
    }
    assert.ok(pages > 0 && pages < 100);
    assert.equal(history.hasMore, false);
    assert.deepEqual(collected.events, before.events);
    assert.deepEqual(collected.runEvents, run.events);
    for (const field of ["actions", "approvals", "artifacts"]) assert.deepEqual(collected[field], run[field]);
    assert.deepEqual(collected.nodes, run.graph.nodes);
    await agent.reconnect();
    const resumedThread = await agent.resumeThread(thread.id);
    assert.deepEqual(resumedThread.configuration, thread.configuration,
      "explicit thread reasoning survives a real process restart");
    const reopenedConfig = await agent.readConfig({threadId: thread.id});
    assert.equal(typeof reopenedConfig.mcpGeneration, "string");
    assert.notEqual(reopenedConfig.mcpGeneration, config.mcpGeneration,
      "a new host owns a new MCP publication, even with no configured servers");
    assert.notEqual(reopenedConfig.configSnapshotId, config.configSnapshotId);
    assert.deepEqual(reopenedConfig, {...config,
      mcpGeneration: reopenedConfig.mcpGeneration, configSnapshotId: reopenedConfig.configSnapshotId},
      "only host publication identity changes; all effective settings and input digests remain exact");
    await assert.rejects(() => thread.run("reject prior host config", {expectedConfigSnapshotId: config.configSnapshotId}),
      /stale execution configuration/u);
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
