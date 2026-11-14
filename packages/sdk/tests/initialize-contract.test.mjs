import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError, HarnessProtocolError, HarnessTransportError } from "../dist/index.js";
const fixture = fileURLToPath(new URL("./initialize-fixture.mjs", import.meta.url));
const valid = () => ({ protocolVersion: 1, serverName: "synthetic-peer", serverVersion: "1",
  capabilities: { threadLifecycle: true, streaming: true, interrupt: true,
    steer: true, approval: true, reconnect: true } });

const invalidEnvelopes = [
  { result: valid(), error: null }, {}, { error: null }, { result: null, error: null },
  { result: valid(), error: { code: -32004, message: "PRIVATE_PAYLOAD" } },
  { result: null, error: { code: -32004, message: "PRIVATE_PAYLOAD" } },
  { error: "PRIVATE_PAYLOAD" }, { error: [] },
];
for (const [index, body] of invalidEnvelopes.entries()) {
  test(`H100 invalid handshake envelope ${index} cannot authorize or use late success`, { timeout: 10000 }, async () => {
    const agent = peer(body, "envelope");
    try {
      await assert.rejects(() => agent.startThread(), (error) => {
        assert.ok(error instanceof HarnessTransportError);
        assert.ok(!error.message.includes("PRIVATE_PAYLOAD"));
        return true;
      });
      assert.equal(agent.transport.initialized, false);
      assert.equal(agent.transport.pending.size, 0);
      assert.equal(agent.transport.child.exitCode, null, "reject before EOF");
      const id = agent.transport.nextId;
      const cause = agent.transport.failure;
      await assert.rejects(() => agent.startThread(), (error) => error === cause);
      await assert.rejects(() => agent.transport.request("thread/list", {}), (error) => error === cause);
      assert.equal(agent.transport.nextId, id, "no late request or automatic retry");
    } finally { await agent.close(); }
  });
  test(`H100 invalid business envelope ${index} settles every pending request`, { timeout: 10000 }, async () => {
    const agent = peer(body, "business-envelope");
    try {
      await agent.transport.initialize({ clientName: "h100", clientVersion: "1", protocolVersion: 1 });
      const results = await Promise.allSettled([
        agent.transport.request("thread/list", {}), agent.transport.request("tool/list", {}),
      ]);
      assert.ok(results.every((result) => result.status === "rejected"));
      const cause = results[0].reason;
      assert.ok(cause instanceof HarnessTransportError);
      assert.ok(!cause.message.includes("PRIVATE_PAYLOAD"));
      assert.equal(results[1].reason, cause);
      assert.equal(agent.transport.initialized, false);
      assert.equal(agent.transport.pending.size, 0);
      assert.equal(agent.transport.child.exitCode, null, "reject before EOF");
      const id = agent.transport.nextId;
      await assert.rejects(() => agent.transport.request("thread/list", {}), (error) => error === cause);
      assert.equal(agent.transport.nextId, id);
    } finally { await agent.close(); }
  });
}
test("H100 valid null/false/zero successes and RPC errors retain the connection", { timeout: 10000 }, async () => {
  for (const body of [{ result: null }, { result: false }, { result: 0 },
    { error: { code: -32004, message: "synthetic rejection" } }]) {
    const agent = peer(body, "business-envelope");
    try {
      await agent.transport.initialize({ clientName: "h100", clientVersion: "1", protocolVersion: 1 });
      if (Object.hasOwn(body, "result")) {
        assert.equal(await agent.transport.request("thread/list", {}), body.result);
      } else {
        await assert.rejects(() => agent.transport.request("thread/list", {}), HarnessRpcError);
      }
      assert.equal(agent.transport.initialized, true);
      assert.equal(agent.transport.failure, undefined);
      assert.deepEqual(await agent.transport.request("audit/count", {}), { business: 1 });
    } finally { await agent.close(); }
  }
});

const cases = [null, [], "PRIVATE_PAYLOAD", {}];
for (const version of [0, 2, 999, -1, 1.5, "1", null]) cases.push({ ...valid(), protocolVersion: version });
for (const field of ["protocolVersion", "serverName", "serverVersion", "capabilities"]) {
  const value = valid(); delete value[field]; cases.push(value);
}
for (const field of ["serverName", "serverVersion"]) {
  for (const value of ["", " \t\n", 17, null]) cases.push({ ...valid(), [field]: value });
}
for (const field of Object.keys(valid().capabilities)) {
  const value = valid(); delete value.capabilities[field]; cases.push(value);
  const wrong = valid(); wrong.capabilities[field] = "PRIVATE_PAYLOAD"; cases.push(wrong);
}
cases.push({ ...valid(), capabilities: [] });
function peer(result, mode) {
  return new DeepAgent({ command: process.execPath, args: [fixture, JSON.stringify(result), mode ?? "result"] });
}
for (const [index, value] of cases.entries()) {
  test(`H206 invalid handshake case ${index} blocks business requests and late duplicates`, { timeout: 10000 }, async () => {
    const agent = peer(value);
    try {
      const barrier = new Promise((resolve) => agent.eventHandlers.add(resolve));
      await assert.rejects(() => agent.startThread(), (error) => {
        assert.ok(error instanceof HarnessProtocolError);
        assert.ok(!error.message.includes("PRIVATE_PAYLOAD"));
        if (typeof value?.protocolVersion === "number" && Number.isSafeInteger(value.protocolVersion) && value.protocolVersion >= 0 && value.protocolVersion !== 1) {
          assert.equal(error.expected, 1);
          assert.equal(error.actual, value.protocolVersion);
        }
        return true;
      });
      await barrier;
      assert.equal(agent.transport.initialized, false);
      assert.equal(agent.transport.pending.size, 0);
      for (const method of ["thread/start", "thread/resume", "thread/list", "thread/read", "thread/fork", "thread/archive",
        "turn/start", "turn/interrupt", "turn/steer", "approval/respond", "event/ack", "tool/list", "config/read", "sandbox/status"]) {
        await assert.rejects(() => agent.requestForTest(method, {}), { name: "HarnessProtocolError" });
      }
      assert.deepEqual(await agent.transport.request("audit/count", {}), { business: 0 });
    } finally { await agent.close(); }
  });
}
test("H206 RPC rejection stays uninitialized after valid late duplicate", async () => {
  const agent = peer(valid(), "rpc-error");
  try {
    const barrier = new Promise((resolve) => agent.eventHandlers.add(resolve));
    await assert.rejects(() => agent.startThread(), HarnessRpcError);
    await barrier;
    assert.equal(agent.transport.initialized, false);
    assert.equal(agent.transport.pending.size, 0);
    assert.deepEqual(await agent.transport.request("audit/count", {}), { business: 0 });
  } finally { await agent.close(); }
});
test("H206 valid identity, false boolean capability and future fields remain compatible", async () => {
  const value = valid(); for (const field of Object.keys(value.capabilities)) value.capabilities[field] = false;
  value.capabilities.future = true; value.future = "extension";
  const agent = peer(value);
  try {
    assert.equal((await agent.startThread()).id, "t");
    assert.equal(agent.transport.initialized, true);
    assert.deepEqual(await agent.transport.request("audit/count", {}), { business: 1 });
  } finally { await agent.close(); }
});
