import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./jsonl-lifecycle-fixture.mjs", import.meta.url));
const regression = { timeout: 12000 };

async function readRecord(file, timeout = 2000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try { return JSON.parse(await readFile(file, "utf8")); }
    catch (error) {
      if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) throw error;
    }
    await delay(10);
  }
  return undefined;
}

function isAlive(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { if (error.code === "ESRCH") return false; throw error; }
}

async function observe(action) {
  const root = await mkdtemp(path.join(os.tmpdir(), "deepagent-jsonl-lifecycle-"));
  const agent = new DeepAgent({ transport: "cli-jsonl", command: process.execPath,
    args: [fixture, action === "malformed" ? "malformed" : "normal", root] });
  const iterator = agent.runJsonl("lifecycle audit")[Symbol.asyncIterator]();
  let pid;
  let released = false;
  const release = async () => {
    await writeFile(path.join(root, "release"), "release test-owned fixture");
    released = true;
  };
  try {
    if (action === "malformed") {
      await assert.rejects(iterator.next(), HarnessTransportError);
    } else if (action === "break") {
      for await (const event of iterator) {
        assert.equal(event.type, "item.updated");
        break;
      }
    } else {
      const first = await iterator.next();
      assert.equal(first.done, false);
      assert.equal(first.value.type, "item.updated");
    }
    const ready = await readRecord(path.join(root, "ready.json"));
    assert.ok(ready, "the real fixture must report its own PID");
    pid = ready.pid;
    let returnDone;
    let remaining = [];
    if (action === "complete") {
      await release();
      for await (const event of iterator) remaining.push(event.type);
    } else if (action === "return") {
      returnDone = (await iterator.return()).done;
      assert.equal(returnDone, true);
    }
    await agent.close();
    // No sleep guesses: the ack proves actual new child work after return,
    // parse failure or close, rather than just an old buffered heartbeat.
    await writeFile(path.join(root, "probe"), "probe after SDK lifecycle boundary");
    const ack = await readRecord(path.join(root, "probe-ack.json"), 250);
    const value = { action, pid, returnDone, remaining,
      aliveAfterClose: isAlive(pid), workAfterClose: ack?.pid === pid };
    console.log(JSON.stringify(value));
    return value;
  } finally {
    if (!released) await release();
    await iterator.return?.();
    await agent.close();
    if (pid === undefined) pid = (await readRecord(path.join(root, "ready.json")))?.pid;
    assert.ok(pid, "cleanup must identify the test-owned child");
    const deadline = Date.now() + 6500;
    while (isAlive(pid) && Date.now() < deadline) await delay(10);
    assert.equal(isAlive(pid), false, "fixture must be gone before assertions escape");
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 25 });
  }
}

test("CLI JSONL fully consumed process exits and performs no work after close", {
  timeout: 12000
}, async () => {
  const value = await observe("complete");
  assert.deepEqual(value.remaining, ["turn.completed"]);
  assert.equal(value.aliveAfterClose, false);
  assert.equal(value.workAfterClose, false);
});

for (const action of ["return", "break", "malformed", "close"]) {
  test(`CLI JSONL ${action} must release its owned child`, regression, async () => {
    const value = await observe(action);
    assert.equal(value.workAfterClose, false, "fixture did new work after SDK lifecycle exit");
    assert.equal(value.aliveAfterClose, false, "SDK left its one-shot CLI child alive");
  });
}
