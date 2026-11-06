import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError, HarnessTransportError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./owned-stream-fixture.mjs", import.meta.url));
function peer(mode) {
  let signal;
  const raw = [];
  const barrier = new Promise((resolve) => { signal = resolve; });
  const agent = new DeepAgent({ command: process.execPath, args: [fixture, mode],
    onEvent: (event) => { raw.push(event); if (event.type === "thread.started" && event.threadId === "version-barrier") signal(); }
  });
  return { agent, barrier, raw };
}

for (const mode of ["version-result", "version-error"]) {
  test(`H212 ${mode}: invalid versions cannot settle pending before a valid reply`, { timeout: 10000 }, async () => {
    const { agent, barrier, raw } = peer(mode);
    try {
      const result = agent.requestForTest("audit/version", {}).then(
        (value) => ({ value }), (error) => ({ error })
      );
      await barrier;
      assert.deepEqual(raw.map((event) => event.type), ["thread.started"]);
      // Inspect actual private transport state, not a reimplemented decoder.
      assert.equal(agent.transport.pending.size, 1);
      await agent.requestForTest("audit/release", {});
      const outcome = await result;
      if (mode === "version-error") {
        assert.ok(outcome.error instanceof HarnessRpcError);
        assert.equal(outcome.error.code, -32004);
        assert.equal(outcome.error.message, "valid matched error");
      } else assert.deepEqual(outcome.value, { source: "valid matched result" });
      assert.equal(agent.transport.pending.size, 0);
    } finally { await agent.close(); }
  });
}

for (const release of [false, true]) {
  test(`H212 initialize cannot authorize on a wrong version; release=${release}`, { timeout: 10000 }, async () => {
    const { agent, barrier, raw } = peer("version-initialize");
    try {
      const result = agent.startThread().then((value) => ({ value }), (error) => ({ error }));
      await barrier;
      assert.deepEqual(raw.map((event) => event.type), ["thread.started"]);
      assert.equal(agent.transport.initialized, false);
      assert.equal(agent.transport.pending.size, 1);
      if (release) {
        // Fixture control uses the same real transport while initialization waits.
        await agent.transport.request("audit/release", {});
        assert.equal((await result).value.id, "thread-1");
        assert.equal(agent.transport.initialized, true);
      } else {
        await agent.close();
        assert.ok((await result).error instanceof HarnessTransportError);
        assert.equal(agent.transport.initialized, false);
      }
      assert.equal(agent.transport.pending.size, 0);
    } finally { await agent.close(); }
  });
}

test("H212 EOF after invalid versions fails the request and clears real pending state", { timeout: 10000 }, async () => {
  const { agent } = peer("version-result");
  try {
    await agent.startThread();
    await assert.rejects(() => agent.requestForTest("audit/version-eof", {}), HarnessTransportError);
    assert.equal(agent.transport.pending.size, 0);
  } finally { await agent.close(); }
});
