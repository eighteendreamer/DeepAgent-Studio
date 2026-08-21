import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import path from "node:path";
import test from "node:test";
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

    const interrupted = await turn.interrupt();
    assert.equal(interrupted.status, "cancelling");

    const steered = await turn.steer("change direction");
    assert.equal(steered.id, "turn-steered");
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
