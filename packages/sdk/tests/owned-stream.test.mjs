import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError, HarnessTransportError, HarnessResultError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./owned-stream-fixture.mjs", import.meta.url));
const bounded = (promise) => Promise.race([promise, delay(300).then(() => "parked")]);

function peer(mode = "hold") {
  const started = new Map();
  const observed = new Set();
  const agent = new DeepAgent({ command: process.execPath, args: [fixture, mode],
    onStderr: (line) => {
      if (line.startsWith("audit:start:")) {
        const thread = line.slice("audit:start:".length);
        observed.add(thread);
        started.get(thread)?.();
      }
    }
  });
  return { agent,
    started: (thread) => observed.has(thread) ? Promise.resolve() : new Promise((resolve) => started.set(thread, resolve)),
    barrier: () => agent.requestForTest("audit/barrier", {}),
    emit: (events) => agent.requestForTest("audit/emit", { events }),
    close: async () => {
      try { await agent.requestForTest("audit/release", {}); } catch { /* peer may have exited */ }
      await agent.close();
    }
  };
}

for (const [mode, expected] of [["early-failed", "turn.failed"], ["early-interrupted", "turn.interrupted"]]) {
  test(`owned stream retains an early ${expected}`, { timeout: 10000 }, async () => {
    const p = peer(mode);
    try {
      const thread = await p.agent.startThread();
      const values = [];
      const task = (async () => { for await (const event of thread.runStreamed("synthetic")) values.push(event); })();
      await p.started(thread.id);
      await p.barrier();
      const before = values.map((event) => event.type);
      await p.agent.requestForTest("audit/release", {});
      await task;
      assert.deepEqual(before, [expected]);
      assert.deepEqual(values.map((event) => event.type), [expected]);
    } finally { await p.close(); }
  });
}

test("bound streams require both thread and turn identity and stop at the first terminal", { timeout: 10000 }, async () => {
  const p = peer();
  try {
    const a = await p.agent.startThread();
    const b = await p.agent.startThread();
    const output = [[], []];
    const tasks = [a, b].map((thread, index) => (async () => {
      for await (const event of thread.runStreamed("concurrent synthetic")) output[index].push(event);
    })());
    await Promise.all([p.started(a.id), p.started(b.id)]);
    await p.barrier();
    await p.emit([
      { type: "turn.completed", threadId: b.id, turnId: `turn-${a.id}`, message: "wrong thread" },
      { type: "approval.requested", threadId: a.id, turnId: `turn-${a.id}`, approvalId: "synthetic-approval", reason: "fixture" },
      { type: "turn.completed", threadId: a.id, turnId: `turn-${a.id}`, message: "first A" },
      { type: "turn.completed", threadId: a.id, turnId: `turn-${a.id}`, message: "duplicate A" },
      { type: "item.updated", threadId: b.id, turnId: `turn-${b.id}`, item: { kind: "content_delta", text: "B" } },
      { type: "turn.completed", threadId: b.id, turnId: `turn-${b.id}`, message: "first B" }
    ]);
    await Promise.all(tasks);
    assert.deepEqual(output[0].map((event) => event.type), ["approval.requested", "turn.completed"]);
    assert.deepEqual(output[1].map((event) => event.type), ["item.updated", "turn.completed"]);
    assert.equal(output[0][1].message, "first A");
    assert.equal(output[1][1].message, "first B");
  } finally { await p.close(); }
});

test("H84 tool gate diagnostic remains a runtime item, not a pending approval", { timeout: 10000 }, async () => {
  const p = peer();
  try {
    const thread = await p.agent.startThread();
    const output = [];
    const task = (async () => {
      for await (const event of thread.runStreamed("synthetic")) output.push(event);
    })();
    await p.started(thread.id);
    const diagnostic = { type: "item.updated", threadId: thread.id, turnId: `turn-${thread.id}`,
      item: { kind: "runtime", eventType: "tool_blocked",
        data: { name: "read_file", reason: "hook ask", needs_approval: true } } };
    await p.emit([diagnostic,
      { type: "turn.completed", threadId: thread.id, turnId: `turn-${thread.id}`, message: "auto allowed" }]);
    await task;
    assert.deepEqual(output.map((event) => event.type), ["item.updated", "turn.completed"]);
    assert.deepEqual(output[0], diagnostic);
  } finally { await p.close(); }
});

for (const mode of ["burst", "large"]) {
  test(`owned stream fails explicitly on ${mode} before identity is known`, { timeout: 10000 }, async () => {
    const p = peer(mode);
    try {
      const thread = await p.agent.startThread();
      const task = (async () => { for await (const _ of thread.runStreamed("bounded")) { /* drain */ } })();
      // Attach the rejection handler before the fixture can overflow.
      const outcome = task.then(() => null, (error) => error);
      await p.started(thread.id);
      await p.barrier();
      await p.agent.requestForTest("audit/release", {});
      const error = await outcome;
      assert.ok(error instanceof HarnessTransportError, `expected bounded failure, got ${error}`);
      assert.match(error.message, /bounded|buffer|overflow/u);
      assert.equal((await p.barrier()).starts.length, 1, "no automatic redispatch");
    } finally { await p.close(); }
  });
}

test("live stream overflow rejects instead of buffering without a bound", { timeout: 10000 }, async () => {
  const p = peer();
  let iterator;
  try {
    const thread = await p.agent.startThread();
    iterator = thread.runStreamed("bounded live")[Symbol.asyncIterator]();
    const first = iterator.next();
    await p.started(thread.id);
    await p.barrier();
    await p.emit(Array.from({ length: 300 }, (_, index) => ({
      type: "item.updated", threadId: thread.id, turnId: `turn-${thread.id}`,
      item: { kind: "content_delta", text: String(index) }
    })));
    assert.equal((await first).done, false);
    const second = await iterator.next().then(() => null, (error) => error);
    assert.ok(second instanceof HarnessTransportError, "overflow must fail the stream");
    assert.equal((await p.barrier()).starts.length, 1);
  } finally {
    await p.close();
    if (iterator) await iterator.return();
  }
});

test("iterator return releases a pending wait without cancelling or repeating the remote turn", { timeout: 10000 }, async () => {
  const p = peer();
  let iterator;
  try {
    const thread = await p.agent.startThread();
    iterator = thread.runStreamed("local return")[Symbol.asyncIterator]();
    const waiting = iterator.next();
    await p.started(thread.id);
    await p.barrier();
    let returned = false;
    const returning = iterator.return().then((value) => { returned = true; return value; });
    await p.barrier();
    const state = await p.barrier();
    const beforeRelease = returned;
    await p.agent.requestForTest("audit/release", {});
    await returning;
    await waiting;
    assert.equal(beforeRelease, true, "return must release a parked next(), not wait for a remote event");
    assert.equal(state.starts.length, 1);
    assert.equal((await iterator.next()).done, true);
  } finally { await p.close(); }
});

test("a pre-aborted stream never submits turn/start or serializes its AbortSignal", { timeout: 10000 }, async () => {
  const p = peer();
  try {
    const thread = await p.agent.startThread();
    const controller = new AbortController();
    controller.abort();
    const iterator = thread.runStreamed("do not start", { signal: controller.signal })[Symbol.asyncIterator]();
    const waiting = iterator.next().then(() => null, (error) => error);
    const state = await p.barrier();
    await p.agent.requestForTest("audit/release", {});
    const error = await waiting;
    await iterator.return();
    assert.ok(error instanceof HarnessTransportError);
    assert.equal(state.starts.length, 0);
  } finally { await p.close(); }
});

test("local cancellation before start response releases the subscriber and never retries", { timeout: 10000 }, async () => {
  const p = peer("hold-start");
  try {
    const thread = await p.agent.startThread();
    const controller = new AbortController();
    const iterator = thread.runStreamed("unknown admitted state", { signal: controller.signal })[Symbol.asyncIterator]();
    const waiting = iterator.next().then(() => null, (error) => error);
    await p.started(thread.id);
    controller.abort();
    await p.barrier();
    const before = await bounded(waiting);
    await p.agent.requestForTest("audit/release", {});
    // The old implementation loses the terminal in the response batch too.
    // A second explicit post-response release only cleans up that red path.
    await p.barrier();
    await p.agent.requestForTest("audit/release", {});
    await waiting;
    await iterator.return();
    assert.ok(before instanceof HarnessTransportError, "cancellation must not await a withheld RPC response");
    const state = await p.barrier();
    assert.equal(state.starts.length, 1);
    assert.equal("signal" in state.starts[0], false);
  } finally { await p.close(); }
});

test("EOF rejects a parked stream after the fixture has definitely exited", { timeout: 10000 }, async () => {
  const p = peer();
  try {
    const thread = await p.agent.startThread();
    const iterator = thread.runStreamed("EOF")[Symbol.asyncIterator]();
    const waiting = iterator.next().then(() => null, (error) => error);
    await p.started(thread.id);
    await p.barrier();
    await assert.rejects(p.agent.requestForTest("audit/exit", {}), HarnessTransportError);
    const outcome = await bounded(waiting);
    assert.ok(outcome instanceof HarnessTransportError, "confirmed EOF must not leave next() parked");
  } finally { await p.agent.close(); }
});

test("rejected starts preserve their RPC errors and allow subsequent explicit requests", { timeout: 10000 }, async () => {
  const p = peer("rpc-error");
  try {
    const thread = await p.agent.startThread();
    for (let index = 0; index < 2; index++) {
      const iterator = thread.runStreamed("busy")[Symbol.asyncIterator]();
      await assert.rejects(iterator.next(), (error) => error instanceof HarnessRpcError && error.code === -32005);
    }
    assert.equal((await p.barrier()).starts.length, 2, "only the two explicit requests");
  } finally { await p.close(); }
});

test("return before the first next is lazy and submits nothing", { timeout: 10000 }, async () => {
  const p = peer();
  try {
    const thread = await p.agent.startThread();
    const iterator = thread.runStreamed("unused")[Symbol.asyncIterator]();
    assert.equal((await iterator.return()).done, true);
    assert.equal((await iterator.next()).done, true);
    assert.equal((await p.barrier()).starts.length, 0);
  } finally { await p.close(); }
});

test("return before admission responds releases pending local consumption", { timeout: 10000 }, async () => {
  const p = peer("hold-start");
  try {
    const thread = await p.agent.startThread();
    const iterator = thread.runStreamed("withheld reply")[Symbol.asyncIterator]();
    const waiting = iterator.next();
    await p.started(thread.id);
    assert.equal((await iterator.return()).done, true);
    assert.equal((await waiting).done, true);
    await p.agent.requestForTest("audit/release", {});
    assert.equal((await p.barrier()).starts.length, 1);
    assert.equal((await iterator.next()).done, true);
  } finally { await p.close(); }
});

test("reconnect fails the old owned stream and does not attach it to the new connection", { timeout: 10000 }, async () => {
  const p = peer();
  try {
    const thread = await p.agent.startThread();
    const old = thread.runStreamed("old generation")[Symbol.asyncIterator]();
    const waiting = old.next().then(() => null, (error) => error);
    await p.started(thread.id);
    await p.barrier();
    await p.agent.reconnect();
    assert.ok(await waiting instanceof HarnessTransportError);
    const current = await p.agent.startThread();
    const events = [];
    const task = (async () => { for await (const event of current.runStreamed("new explicit request")) events.push(event); })();
    // Fresh peer reuses synthetic IDs deliberately; the old lifetime must not.
    await p.barrier();
    await p.barrier();
    await p.agent.requestForTest("audit/release", {});
    await task;
    assert.deepEqual(events.map((event) => event.type), ["turn.completed"]);
    assert.equal((await p.barrier()).starts.length, 1);
    await assert.rejects(old.next(), HarnessTransportError);
  } finally { await p.close(); }
});

test("close rejects a live owned stream instead of parking next forever", { timeout: 10000 }, async () => {
  const p = peer();
  try {
    const thread = await p.agent.startThread();
    const iterator = thread.runStreamed("close")[Symbol.asyncIterator]();
    const waiting = iterator.next().then(() => null, (error) => error);
    await p.started(thread.id);
    await p.barrier();
    await p.agent.close();
    assert.ok(await waiting instanceof HarnessTransportError);
  } finally { await p.agent.close(); }
});

test("an unsupported JSONL owned stream rejects through the iterator without starting a child", async () => {
  const agent = new DeepAgent({ transport: "cli-jsonl", command: "__h214_never_spawn__" });
  const { Thread } = await import("../dist/index.js");
  const iterator = new Thread(agent, "synthetic").runStreamed("unsupported");
  await assert.rejects(iterator.next(), HarnessTransportError);
  assert.equal((await iterator.return()).done, true);
  await agent.close();
});

test("pre-response candidates are correlated only after admission, including v1 optional thread context", { timeout: 10000 }, async () => {
  const p = peer("candidate-isolation");
  try {
    const thread = await p.agent.startThread();
    const events = [];
    for await (const event of thread.runStreamed("synthetic")) events.push(event);
    assert.deepEqual(events.map((event) => event.type), ["item.updated", "turn.completed"]);
    assert.equal(events[0].item.text, "original");
    assert.equal((await p.barrier()).starts.length, 1);
  } finally { await p.close(); }
});

for (const mode of ["invalid-thread", "invalid-turn"]) {
  test(`owned stream rejects ${mode} admission without leaking its value or redispatching`, { timeout: 10000 }, async () => {
    const p = peer(mode);
    try {
      const thread = await p.agent.startThread();
      const iterator = thread.runStreamed("synthetic");
      await assert.rejects(iterator.next(), (error) => error instanceof HarnessResultError &&
        !error.message.includes("PRIVATE_BAD_ID"));
      assert.equal((await p.barrier()).starts.length, 1);
      await iterator.return();
    } finally { await p.close(); }
  });
}

test("a buffered original terminal remains deliverable when stdout closes in the admission batch", { timeout: 10000 }, async () => {
  const p = peer("terminal-eof");
  try {
    const thread = await p.agent.startThread();
    const events = [];
    for await (const event of thread.runStreamed("synthetic")) events.push(event);
    assert.deepEqual(events.map((event) => event.type), ["turn.completed"]);
    assert.equal(events[0].message, "original terminal");
  } finally { await p.agent.close(); }
});
