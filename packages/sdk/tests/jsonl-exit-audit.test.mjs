import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./jsonl-exit-fixture.mjs", import.meta.url));
const regression = { timeout: 10000 };

async function waitForRecord(recordPath) {
  const deadline = Date.now() + 6500;
  while (Date.now() < deadline) {
    try {
      return JSON.parse(await readFile(recordPath, "utf8"));
    } catch (error) {
      if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) throw error;
    }
    await delay(10);
  }
  throw new Error(`fixture did not produce ${path.basename(recordPath)}`);
}

async function observe(mode) {
  const root = await mkdtemp(path.join(os.tmpdir(), "deepagent-jsonl-exit-"));
  const recordPath = path.join(root, "exit.json");
  const releasePath = path.join(root, "release");
  const stderr = [];
  const events = [];
  let failure;
  let settled = false;
  let completedBeforeRelease;
  let aliveBeforeRelease;
  const agent = new DeepAgent({
    transport: "cli-jsonl", command: process.execPath,
    args: [fixture, mode, recordPath, releasePath],
    onStderr: (line) => stderr.push(line)
  });
  const consumption = (async () => {
    try {
      for await (const event of agent.runJsonl("audit prompt")) events.push(event);
    } catch (error) {
      failure = error;
    } finally {
      settled = true;
    }
  })();
  try {
    if (mode === "early-eof") {
      try {
        const ready = await waitForRecord(`${recordPath}.ready`);
        // Bound the observation, not the fixture's exit. The file gate stays
        // closed throughout, so EOF cannot be mistaken for process exit.
        await Promise.race([consumption, delay(200)]);
        completedBeforeRelease = settled;
        process.kill(ready.pid, 0);
        aliveBeforeRelease = true;
      } finally {
        await writeFile(releasePath, "release test-owned fixture");
      }
    }
    await consumption;
    const exit = await waitForRecord(recordPath);
    const value = { mode, exit, events: events.map((event) => event.type),
      stderr, completedBeforeRelease, aliveBeforeRelease,
      error: failure && { name: failure.name, message: failure.message } };
    console.log(JSON.stringify(value));
    return { ...value, failure };
  } finally {
    // Always release a gated fixture, even when a barrier or assertion fails.
    await writeFile(releasePath, "release test-owned fixture");
    await agent.close();
    await waitForRecord(recordPath);
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 25 });
  }
}

test("CLI JSONL success preserves events and exits zero", { timeout: 10000 }, async () => {
  const value = await observe("success");
  assert.equal(value.exit.code, 0);
  assert.deepEqual(value.events, ["item.updated", "turn.completed"]);
  assert.equal(value.failure, undefined);
});

for (const mode of ["partial-failure", "empty-failure"]) {
  test(`CLI JSONL ${mode} must reject a nonzero child exit`, regression, async () => {
    const value = await observe(mode);
    assert.equal(value.exit.code, 23, "fixture exit hook must record the injected code");
    assert.deepEqual(value.events, mode === "partial-failure" ? ["item.updated"] : []);
    assert.deepEqual(value.stderr, ["audit fixture failure: code 23"]);
    assert.ok(value.failure instanceof HarnessTransportError,
      "SDK silently ended the iteration despite a nonzero child exit");
    assert.deepEqual(value.failure.cause, { code: 23, signal: null, stderrPresent: true });
  });
}

test("CLI JSONL stdout EOF must not finish before the child exits", regression, async () => {
  const value = await observe("early-eof");
  assert.equal(value.aliveBeforeRelease, true);
  assert.equal(value.exit.code, 23);
  assert.equal(value.completedBeforeRelease, false,
    "SDK finished while its test-owned CLI child was alive behind the release gate");
  assert.ok(value.failure instanceof HarnessTransportError);
  assert.deepEqual(value.failure.cause, { code: 23, signal: null, stderrPresent: false });
});
