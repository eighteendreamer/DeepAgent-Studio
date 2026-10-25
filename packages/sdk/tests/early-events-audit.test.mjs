import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./early-events-fixture.mjs", import.meta.url));
const auditOnly = { skip: process.env.RUN_HARNESS_AUDIT !== "1", timeout: 10000 };

async function observe(mode) {
  const raw = [];
  const streamed = [];
  let resolveStarted;
  const started = new Promise((resolve) => { resolveStarted = resolve; });
  const agent = new DeepAgent({ command: process.execPath, args: [fixture, mode],
    onEvent: (event) => raw.push(event),
    onStderr: (line) => {
      if (line === "audit: turn/start processed") resolveStarted();
    } });
  let consumption;
  let finished = false;
  try {
    const thread = await agent.startThread();
    consumption = (async () => {
      for await (const event of thread.runStreamed("early audit")) streamed.push(event);
      finished = true;
    })();
    await started;
    assert.deepEqual(await agent.requestForTest("audit/barrier", {}), { started: true });
    // A round trip behind turn/start makes the original inbound delivery and
    // its promise continuations observable without guessing a server delay.
    const beforeRelease = { raw: raw.map((event) => event.type),
      streamed: streamed.map((event) => event.type), finished };
    await agent.requestForTest("audit/release", {});
    await consumption;
    const result = { mode, beforeRelease,
      raw: raw.map((event) => ({ type: event.type, message: event.message })),
      streamed: streamed.map((event) => ({ type: event.type, message: event.message })) };
    console.log(JSON.stringify(result));
    return result;
  } finally {
    if (consumption && !finished) {
      // Release a parked runStreamed before attempting generator/transport close.
      await agent.requestForTest("audit/release", {});
      await Promise.race([consumption, delay(500)]);
    }
    await assert.rejects(agent.requestForTest("audit/exit", {}), HarnessTransportError);
    await agent.close();
  }
}

test("runStreamed preserves events delivered after the start continuation", {
  timeout: 10000
}, async () => {
  const value = await observe("controlled");
  assert.deepEqual(value.beforeRelease.raw, []);
  assert.deepEqual(value.streamed.map((event) => event.type), ["item.updated", "turn.completed"]);
  assert.equal(value.streamed[1].message, "original terminal");
});

for (const mode of ["event-first", "response-first"]) {
  test(`runStreamed preserves a single-write ${mode} event burst`, auditOnly, async () => {
    const value = await observe(mode);
    assert.deepEqual(value.beforeRelease.raw, ["item.updated", "turn.completed"],
      "the public transport observer must receive both original events");
    assert.deepEqual(value.beforeRelease.streamed, ["item.updated", "turn.completed"],
      "the high-level stream discarded events that reached the same SDK");
    assert.equal(value.beforeRelease.finished, true, "the original terminal must finish the stream");
  });
}
