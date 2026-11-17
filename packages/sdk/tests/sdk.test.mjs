import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import path from "node:path";
import test from "node:test";
import { readFileSync } from "node:fs";
import { DeepAgent, HarnessRpcError } from "../dist/index.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const mockServer = path.join(__dirname, "mock-server.mjs");
const mockCli = path.join(__dirname, "mock-cli.mjs");

function createAgent(options = {}) {
  return new DeepAgent({
    transport: "stdio",
    command: process.execPath,
    args: [mockServer],
    ...options
  });
}

test("public fork wrapper preserves the existing request and receipt", async () => {
  const agent = createAgent();
  try {
    const thread = await agent.startThread();
    const receipt = await agent.forkThread(thread.id, { atSequence: 0 });
    assert.equal(receipt.sourceThreadId, thread.id);
    assert.equal(receipt.threadId, "thread-forked");
    assert.equal(receipt.forkedAt, 0);
    assert.deepEqual(receipt.restoredPaths, []);
    assert.deepEqual(receipt.request, { threadId: thread.id, atSequence: 0 });
    const latest = await agent.forkThread(thread.id);
    assert.deepEqual(latest.request, { threadId: thread.id });
  } finally { await agent.close(); }
});

test("public sandbox wrapper reads capabilities through the existing transport", async () => {
  const agent = createAgent();
  try {
    const status = await agent.sandboxStatus();
    assert.equal(status.kind, "direct");
    assert.equal(status.available, true);
    assert.equal(status.supports_readonly_mapping, false);
    assert.equal(status.coordinator.ready, true);
    assert.deepEqual(status.request, {});
  } finally { await agent.close(); }
});

test("public ACK wrapper preserves the receipt and rejects lossy local sequences", async () => {
  const agent = createAgent();
  try {
    assert.deepEqual(await agent.acknowledgeEvents(7), {
      acknowledgedEventSequence: 7, status: "accepted"
    });
    for (const sequence of [-1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) {
      await assert.rejects(() => agent.acknowledgeEvents(sequence), /safe unsigned integer/u);
      await assert.rejects(() => agent.forkThread("thread-mock", { atSequence: sequence }), /safe unsigned integer/u);
    }
  } finally { await agent.close(); }
});

test("H127 preserves ordered tool image refs from the real Rust protocol fixture", async () => {
  const fixture = JSON.parse(readFileSync(new URL("../../../crates/deepagent-harness-protocol/tests/fixtures/tool-image-result-v1.json", import.meta.url), "utf8"));
  const agent = createAgent();
  try {
    const thread = await agent.startThread();
    const events = [];
    for await (const event of thread.runStreamed("fixture:h127-tool-image")) events.push(event);
    const completed = events.find(event => event.type === "item.completed");
    assert.deepEqual(completed.item, fixture.item);
    assert.equal(completed.item.content[1].attachment.sha256, "a".repeat(64));
    assert.equal(JSON.stringify(completed).includes("base64"), false);
  } finally { await agent.close(); }
});

test("forwards explicit thread reasoning and exposes the saved value after resume", async () => {
  const agent = createAgent();
  try {
    for (const effort of ["none", "low", "high", "max"]) {
      const thread = await agent.startThread({reasoningEffort: effort});
      assert.equal(thread.configuration.overrides.reasoning_effort, effort);
      assert.deepEqual((await agent.resumeThread(thread.id)).configuration, thread.configuration);
    }
  } finally { await agent.close(); }
});

test("reads host/thread/cwd configuration through the existing stdio transport", async () => {
  const agent = createAgent();
  try {
    for (const scope of [{}, {threadId: "thread-mock"}, {cwd: process.cwd()}]) {
      const config = await agent.readConfig(scope);
      assert.equal(config.configSnapshotId, "config-mock");
      assert.deepEqual(config.request, scope);
    }
  } finally { await agent.close(); }
});

test("reads one typed history page without automatic accumulation or retry", async () => {
  const agent = createAgent();
  try {
    const first = await agent.readThreadPage("thread-mock", {
      pageSize: 7, maxBytes: 8192, sessionAfterSequence: 2, runAfterSequences: { "run-1": 3 }
    });
    assert.equal(first.hasMore, true);
    assert.deepEqual(first.events[0].request, {
      threadId: "thread-mock", pageSize: 7, maxBytes: 8192,
      sessionAfterSequence: 2, runAfterSequences: { "run-1": 3 }
    });
    const second = await agent.readThreadPage("thread-mock", { cursor: first.nextCursor });
    assert.equal(second.hasMore, false);
    assert.equal(second.nextCursor, null);
    await assert.rejects(() => agent.readThreadPage("legacy-history"), /bounded history page/u);
  } finally { await agent.close(); }
});

test("starts a thread, streams events, and resolves approvals", async () => {
  const approvals = [];
  const agent = createAgent({
    onApproval: async (request) => {
      approvals.push(request);
      return true;
    }
  });

  try {
    const thread = await agent.startThread({ cwd: process.cwd() });
    assert.equal(thread.id, "thread-mock");

    const events = [];
    for await (const event of thread.runStreamed("hello")) {
      events.push(event);
    }

    assert.deepEqual(
      events.map((event) => event.type),
      ["item.updated", "approval.requested", "turn.completed"]
    );
    assert.equal(events[0].item.kind, "content_delta");
    assert.equal(approvals[0].approvalId, "approval-mock");
  } finally {
    await agent.close();
  }
});

test("interrupts and steers a running turn with stable request shapes", async () => {
  const agent = createAgent();
  try {
    const thread = await agent.startThread();
    const turn = await thread.run("long task");
    assert.equal(turn.snapshotId, "snapshot-start-mock");
    assert.equal(turn.capabilitySnapshot.runId, turn.id);
    assert.equal(turn.capabilitySnapshot.threadId, thread.id);

    const interrupted = await turn.interrupt();
    assert.equal(interrupted.status, "cancelling");

    const steered = await turn.steer("change direction");
    assert.equal(steered.id, "turn-steered");
    assert.equal(steered.snapshotId, "snapshot-steer-mock");
    assert.equal(steered.capabilitySnapshot.runId, steered.id);
  } finally {
    await agent.close();
  }
});

test("reconnects the stdio process and resumes a thread", async () => {
  const agent = createAgent();
  try {
    const thread = await agent.startThread();
    await agent.reconnect();
    const resumed = await agent.resumeThread(thread.id);
    assert.equal(resumed.id, thread.id);
  } finally {
    await agent.close();
  }
});

test("exposes saved configuration on start/resume and distinguishes legacy metadata", async () => {
  const agent = createAgent();
  try {
    const older = await agent.startThread();
    assert.equal(older.configuration, undefined);
    assert.equal((await agent.resumeThread(older.id)).configuration, null);
    const thread = await agent.startThread({ permissionProfile: "read_only" });
    const expected = { version: 1, overrides: { permission_profile: "read_only" } };
    assert.deepEqual(thread.configuration, expected);
    assert.deepEqual((await agent.resumeThread(thread.id)).configuration, expected);
  } finally {
    await agent.close();
  }
});

test("lists global tools and thread-scoped run capabilities", async () => {
  const agent = createAgent();
  try {
    const thread = await agent.startThread();
    const global = await agent.listTools();
    assert.deepEqual(global.tools, [{ name: "read_file" }]);
    assert.equal(global.snapshotId, undefined);

    const scoped = await thread.listTools("run-1");
    assert.equal(scoped.threadId, thread.id);
    assert.equal(scoped.runId, "run-1");
    assert.equal(scoped.snapshotId, "snapshot-mock");
    const checked = await agent.listTools({threadId: thread.id, runId: "run-1", expectedSnapshotId: scoped.snapshotId});
    assert.equal(checked.snapshotId, scoped.snapshotId);
    await assert.rejects(() => agent.listTools({threadId: thread.id, expectedSnapshotId: "stale-snapshot"}),
      (error) => error instanceof HarnessRpcError && error.code === -32602);
  } finally {
    await agent.close();
  }
});

test("maps JSON-RPC errors to a stable SDK error", async () => {
  const agent = createAgent();
  try {
    const thread = await agent.startThread();
    await assert.rejects(
      () => agent.requestForTest("missing/method", {}),
      (error) => {
        assert.ok(error instanceof HarnessRpcError);
        assert.equal(error.code, -32601);
        return true;
      }
    );
    assert.equal(thread.id, "thread-mock");
  } finally {
    await agent.close();
  }
});

test("busy start rejects without a phantom turn or automatic retry", async () => {
  const agent = createAgent();
  try {
    const thread = await agent.startThread();
    await assert.rejects(() => thread.run("fixture:busy"), (error) => {
      assert.ok(error instanceof HarnessRpcError);
      assert.equal(error.code, -32005);
      return true;
    });
    const events = [];
    for await (const event of thread.runStreamed("normal after rejection")) {
      events.push(event);
    }
    assert.equal(events.at(-1).type, "turn.completed");
    assert.equal(events.at(-1).turnId, "turn-mock-1");
  } finally {
    await agent.close();
  }
});

test("streams one-shot CLI JSONL events without a server session", async () => {
  const agent = new DeepAgent({
    transport: "cli-jsonl",
    command: process.execPath,
    args: [mockCli]
  });
  const events = [];
  for await (const event of agent.runJsonl("hello")) {
    events.push(event);
  }
  assert.deepEqual(
    events.map((event) => event.type),
    ["item.updated", "turn.completed"]
  );
});
