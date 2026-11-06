import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./approval-isolation-fixture.mjs", import.meta.url));
const options = { timeout: 12000 };
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function observed(task) {
  const state = { settled: false };
  state.task = task.then((value) => { state.settled = true; state.value = value; },
    (error) => { state.settled = true; state.error = error; });
  return state;
}
function peer(mode = "normal", handler = "hold") {
  const answers = new Map(), requests = [], children = new Map(), streams = [];
  const entered = new Set(), started = new Map();
  const agent = new DeepAgent({ command: process.execPath, args: [fixture, mode],
    onStderr: (line) => {
      if (!line.startsWith("audit:started:")) return;
      const id = line.slice("audit:started:".length); entered.add(id); started.get(id)?.();
    },
    onApproval: handler === "none" ? undefined : (request) => {
      requests.push(request);
      if (handler === "throw") throw new Error("synthetic handler exception");
      if (handler === "deny") return false;
      const gate = deferred(); answers.set(request.approvalId, gate); return gate.promise;
    }
  });
  const capture = () => {
    const child = agent.transport.child;
    if (!child || children.has(child)) return;
    children.set(child, child.exitCode !== null || child.signalCode !== null
      ? Promise.resolve() : new Promise((resolve) => child.once("exit", resolve)));
  };
  const barrier = () => agent.requestForTest("audit/barrier", {});
  const emit = (...events) => agent.requestForTest("audit/emit", { events });
  const thread = async () => { const value = await agent.startThread(); capture(); return value; };
  const stream = async (value, extra) => {
    const iterator = value.runStreamed("synthetic client lifecycle", extra);
    const events = [];
    const state = observed((async () => { for await (const event of iterator) events.push(event); })());
    streams.push({ iterator, state });
    if (!entered.has(value.id)) await new Promise((resolve) => started.set(value.id, resolve));
    const snapshot = await barrier();
    const turnId = snapshot.turns.find((turn) => turn.threadId === value.id).turnId;
    return { iterator, state, events, threadId: value.id, turnId };
  };
  const approval = (owner, approvalId, scope = {}) => ({ type: "approval.requested",
    threadId: owner.threadId, turnId: owner.turnId, approvalId, reason: "synthetic approval", ...scope });
  const terminal = (owner, type = "turn.completed") => ({ type,
    threadId: owner.threadId, turnId: owner.turnId, message: "original terminal", reason: "original stop" });
  const close = async () => {
    for (const gate of answers.values()) gate.resolve(false);
    try { await agent.requestForTest("audit/release-receipts", {}); } catch { /* closed generation */ }
    await agent.waitForPendingApprovals();
    for (const { iterator } of streams) await iterator.return();
    await Promise.all(streams.map(({ state }) => state.task));
    capture(); await agent.close(); await Promise.all(children.values());
    for (const child of children.keys()) {
      assert.throws(() => process.kill(child.pid, 0), { code: "ESRCH" }, "test-owned peer must actually exit");
    }
  };
  return { agent, answers, requests, thread, stream, approval, terminal, barrier, emit, close };
}

for (const type of ["turn.completed", "turn.failed", "turn.interrupted"]) {
  test(`H295 an unrelated pending approval cannot delay ${type} consumption`, options, async () => {
    const p = peer(); try {
      const a = await p.stream(await p.thread()), b = await p.stream(await p.thread());
      await p.emit(p.approval(a, "held-A")); await p.barrier();
      const drain = observed(p.agent.waitForPendingApprovals());
      await p.emit(p.terminal(b, type)); const before = await p.barrier();
      assert.equal(b.state.settled, true);
      assert.equal(a.state.settled, false);
      assert.equal(drain.settled, false, "explicit global drain still owns A's callback");
      assert.deepEqual(before.responses, []);
      assert.deepEqual(b.events.map((event) => event.type), [type]);
      p.answers.get("held-A").resolve(false); await drain.task;
      assert.equal((await p.barrier()).responses.length, 1);
    } finally { await p.close(); }
  });
}

test("H295 even same-turn callbacks are separate from authoritative terminal consumption", options, async () => {
  const p = peer(); try {
    const a = await p.stream(await p.thread()), b = await p.stream(await p.thread());
    await p.emit(p.approval(a, "held-A"), p.approval(b, "held-B")); await p.barrier();
    const drain = observed(p.agent.waitForPendingApprovals());
    await p.emit(p.terminal(a), p.terminal(b)); const before = await p.barrier();
    assert.equal(a.state.settled, true); assert.equal(b.state.settled, true);
    assert.deepEqual(before.responses, []); assert.equal(drain.settled, false);
    p.answers.get("held-A").resolve(false); await p.barrier(); assert.equal(drain.settled, false);
    p.answers.get("held-B").resolve(false); await drain.task;
    const after = await p.barrier(); assert.equal(after.responses.length, 2);
    assert.ok(after.responses.every((response) => response.approved === false));
  } finally { await p.close(); }
});

for (const scope of [{ threadId: undefined, turnId: undefined }, { threadId: undefined }, { turnId: undefined }]) {
  test(`H295 optional approval identity ${Object.keys(scope).join("+")} does not become a global stream barrier`, options, async () => {
    const p = peer(); try {
      const b = await p.stream(await p.thread());
      await p.emit(p.approval(b, "unscoped", scope)); await p.barrier();
      assert.equal(p.requests.length, 1);
      for (const key of Object.keys(scope)) assert.equal(p.requests[0][key], undefined);
      const drain = observed(p.agent.waitForPendingApprovals());
      await p.emit(p.terminal(b)); const before = await p.barrier();
      assert.equal(b.state.settled, true); assert.equal(drain.settled, false);
      assert.deepEqual(before.responses, []);
      assert.equal(b.events.filter((event) => event.type === "turn.completed").length, 1);
    } finally { await p.close(); }
  });
}

test("H295 terminal consumption does not wait for its own approval response receipt", options, async () => {
  const p = peer("hold-receipt", "deny"); try {
    const a = await p.stream(await p.thread());
    await p.emit(p.approval(a, "own-receipt")); await p.barrier();
    assert.equal((await p.barrier()).withheld, 1);
    const drain = observed(p.agent.waitForPendingApprovals());
    await p.emit(p.terminal(a)); await p.barrier();
    assert.equal(a.state.settled, true); assert.equal(drain.settled, false);
    await p.agent.requestForTest("audit/release-receipts", {}); await drain.task;
    assert.equal((await p.barrier()).responses.length, 1);
  } finally { await p.close(); }
});

test("H295 pre-admission approval and terminal batch retains both events without implicit drain", options, async () => {
  const p = peer("early"); try {
    const a = await p.stream(await p.thread()); await p.barrier();
    assert.equal(a.state.settled, true);
    assert.deepEqual(a.events.map((event) => event.type), ["approval.requested", "turn.completed"]);
    assert.equal(p.requests.length, 1); assert.deepEqual((await p.barrier()).responses, []);
    const drain = observed(p.agent.waitForPendingApprovals()); await p.barrier();
    assert.equal(drain.settled, false);
  } finally { await p.close(); }
});

for (const action of ["return", "abort"]) {
  test(`H295 ${action} releases only local consumption, not a held approval`, options, async () => {
    const p = peer(); try {
      const controller = new AbortController();
      const a = await p.stream(await p.thread(), { signal: controller.signal });
      await p.emit(p.approval(a, "local-held")); await p.barrier();
      const drain = observed(p.agent.waitForPendingApprovals());
      if (action === "return") await a.iterator.return(); else controller.abort();
      await a.state.task; const before = await p.barrier();
      assert.equal(a.state.settled, true); assert.equal(drain.settled, false);
      if (action === "abort") assert.ok(a.state.error instanceof HarnessTransportError);
      assert.deepEqual(before.responses, []);
    } finally { await p.close(); }
  });
}

for (const action of ["close", "reconnect"]) {
  test(`H295 ${action} leaves explicit callback ownership intact and cannot answer on a new generation`, options, async () => {
    const p = peer(); try {
      const a = await p.stream(await p.thread());
      await p.emit(p.approval(a, "old-held")); await p.barrier();
      const drain = observed(p.agent.waitForPendingApprovals());
      if (action === "close") await p.agent.close(); else await p.agent.reconnect();
      await a.state.task; assert.ok(a.state.error instanceof HarnessTransportError);
      assert.equal(drain.settled, false);
      if (action === "close") await p.agent.reconnect();
      const current = await p.stream(await p.thread());
      await p.emit(p.terminal(current)); await p.barrier();
      assert.equal(current.state.settled, true);
      p.answers.get("old-held").resolve(false); await drain.task;
      assert.deepEqual((await p.barrier()).responses, [], "old approval never crosses the transport generation");
    } finally { await p.close(); }
  });
}

for (const [mode, handler] of [["normal", "none"], ["normal", "throw"], ["rpc-error", "deny"], ["invalid-receipt", "deny"]]) {
  test(`H295 ${mode}/${handler} preserves the existing approval settlement policy`, options, async () => {
    const p = peer(mode, handler); try {
      const a = await p.stream(await p.thread());
      await p.emit(p.approval(a, "settled")); await p.barrier();
      await p.agent.waitForPendingApprovals(); await p.emit(p.terminal(a)); await p.barrier();
      assert.equal(a.state.settled, true); assert.equal(a.state.error, undefined);
      const { responses } = await p.barrier();
      assert.equal(responses.length, handler === "none" ? 0 : 1);
      assert.ok(responses.every((response) => response.approved === false));
    } finally { await p.close(); }
  });
}

test("H295 explicit global drain remains a snapshot, not an implicit future-work barrier", options, async () => {
  const p = peer(); try {
    const a = await p.stream(await p.thread()), b = await p.stream(await p.thread());
    await p.emit(p.approval(a, "first")); await p.barrier();
    const firstDrain = observed(p.agent.waitForPendingApprovals());
    await p.emit(p.approval(b, "later")); await p.barrier();
    const allDrain = observed(p.agent.waitForPendingApprovals());
    p.answers.get("first").resolve(false); await firstDrain.task; await p.barrier();
    assert.equal(allDrain.settled, false);
    p.answers.get("later").reject(new Error("synthetic decision rejected")); await allDrain.task;
    assert.equal((await p.barrier()).responses.length, 2);
  } finally { await p.close(); }
});
