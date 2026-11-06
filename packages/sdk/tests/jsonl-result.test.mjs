import assert from "node:assert/strict";
import { once } from "node:events";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./jsonl-result-fixture.mjs", import.meta.url));
const options = { timeout: 15000 };
async function record(file) {
  const deadline = Date.now() + 3000;
  while (Date.now() < deadline) {
    try { return JSON.parse(await readFile(file, "utf8")); }
    catch (error) { if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) throw error; }
    await delay(10);
  }
  throw new Error("test-owned JSONL process did not acknowledge its gate");
}
async function withResult(mode, id, body, extra = {}) {
  const prefix = path.join(os.tmpdir(), "h266-result-");
  const root = await mkdtemp(prefix);
  const stderr = [];
  const agent = new DeepAgent({ transport: "cli-jsonl", command: process.execPath,
    args: [fixture, root, mode], onStderr: (line) => stderr.push(line), ...extra });
  const iterator = agent.runJsonl(id)[Symbol.asyncIterator]();
  const file = (suffix) => path.join(root, `${id}.${suffix}`);
  const release = () => writeFile(file("release"), "release synthetic process");
  try { await body({ agent, iterator, file, release, stderr }); }
  finally {
    await release();
    await iterator.return().catch(() => {});
    await agent.close();
    if (existsSync(file("ready.json"))) {
      const { pid } = await record(file("ready.json"));
      assert.throws(() => process.kill(pid, 0), { code: "ESRCH" }, "fixture root must have actually exited");
    }
    assert.ok(root.startsWith(prefix));
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 25 });
  }
}
async function collect(iterator) {
  const events = [];
  try { for await (const event of iterator) events.push(event); return { events }; }
  catch (failure) { return { events, failure }; }
}

for (const mode of ["empty", "partial", "turn.completed", "turn.failed", "turn.interrupted"]) {
  test(`H266 nonzero ${mode} retains delivered events but rejects the process result`, options, async () => {
    await withResult(mode, "nonzero-result", async ({ iterator, file, release, stderr }) => {
      const consuming = collect(iterator);
      await record(file("ready.json")); await release();
      const result = await consuming;
      assert.equal((await record(file("exit.json"))).code, 23);
      assert.deepEqual(result.events.map((event) => event.type), mode === "empty" ? [] : [mode === "partial" ? "item.updated" : mode]);
      assert.ok(result.failure instanceof HarnessTransportError);
      assert.deepEqual(result.failure.cause, { code: 23, signal: null, stderrPresent: true });
      assert.match(result.failure.message, /code=23, signal=null/u);
      assert.ok(!JSON.stringify(result.failure).includes("synthetic-private-stderr-canary"));
      assert.ok(!result.failure.message.includes("synthetic-private-stderr-canary"));
      assert.deepEqual(stderr, ["synthetic-private-stderr-canary"]);
    });
  });
}

for (const mode of ["empty", "partial", "turn.completed", "turn.failed", "turn.interrupted"]) {
  test(`H266 zero ${mode} remains a raw event projection, not a fabricated business success`, options, async () => {
    await withResult(mode, "zero-result", async ({ iterator, file, release }) => {
      const consuming = collect(iterator);
      await record(file("ready.json")); await release();
      const result = await consuming;
      assert.equal((await record(file("exit.json"))).code, 0);
      assert.equal(result.failure, undefined);
      assert.deepEqual(result.events.map((event) => event.type), mode === "empty" ? [] : [mode === "partial" ? "item.updated" : mode]);
    });
  });
}

test("H266 terminal output cannot finish before native exit and trailing stderr", options, async () => {
  await withResult("turn.completed", "nonzero-gated", async ({ agent, iterator, file, release, stderr }) => {
    assert.equal((await iterator.next()).value.type, "turn.completed");
    const { pid } = await record(file("ready.json")); process.kill(pid, 0);
    let settled = false;
    const next = iterator.next().then((value) => ({ value }), (failure) => ({ failure })).finally(() => { settled = true; });
    await delay(100); assert.equal(settled, false); assert.deepEqual(stderr, []);
    const owner = [...agent.cliTransport.active][0]; const closed = once(owner.child, "close");
    await release();
    const result = await next;
    assert.ok(result.failure instanceof HarnessTransportError);
    assert.deepEqual(await closed, [23, null]);
    assert.deepEqual(stderr, ["synthetic-private-stderr-canary"]);
  });
});

test("H266 an externally signalled real child fails with the native receipt", options, async () => {
  await withResult("signal", "native-signal", async ({ agent, iterator, file }) => {
    await iterator.next(); await record(file("ready.json"));
    const owner = [...agent.cliTransport.active][0]; const closed = once(owner.child, "close");
    const consuming = collect(iterator);
    assert.equal(owner.child.kill("SIGKILL"), true);
    const [code, signal] = await closed;
    assert.ok(code !== 0 || signal !== null);
    console.log(JSON.stringify({ label: "H266 native external termination", code, signal }));
    const result = await consuming;
    assert.ok(result.failure instanceof HarnessTransportError);
    assert.deepEqual(result.failure.cause, { code, signal, stderrPresent: false });
  });
});

test("H266 close after a natural failed exit cannot erase its result", options, async () => {
  await withResult("partial", "nonzero-before-close", async ({ agent, iterator, file, release }) => {
    await iterator.next(); await record(file("ready.json"));
    const closed = once([...agent.cliTransport.active][0].child, "close");
    await release(); assert.deepEqual(await closed, [23, null]);
    await agent.close();
    const result = await collect(iterator);
    assert.ok(result.failure instanceof HarnessTransportError);
    assert.equal(result.failure.cause.code, 23);
  });
});

test("H266 process results are invocation-local when one succeeds and one fails", options, async () => {
  await withResult("partial", "nonzero-first", async ({ agent, iterator, file, release }) => {
    const failing = collect(iterator); await record(file("ready.json"));
    const succeeding = collect(agent.runJsonl("zero-second"));
    const sibling = file("ready.json").replace("nonzero-first.ready.json", "zero-second.ready.json");
    await record(sibling);
    await writeFile(sibling.replace("ready.json", "release"), "release success only");
    assert.equal((await succeeding).failure, undefined);
    await release(); assert.ok((await failing).failure instanceof HarnessTransportError);
    const afterFailure = collect(agent.runJsonl("zero-after-failure"));
    const next = sibling.replace("zero-second.ready.json", "zero-after-failure.ready.json");
    await record(next);
    await writeFile(next.replace("ready.json", "release"), "release later success");
    assert.equal((await afterFailure).failure, undefined, "a failed invocation does not close the shared transport");
  });
});

test("H266 stderr presence is safe metadata even without a callback", options, async () => {
  await withResult("empty", "nonzero-no-callback", async ({ iterator, file, release }) => {
    const consuming = collect(iterator); await record(file("ready.json")); await release();
    const { failure } = await consuming;
    assert.ok(failure instanceof HarnessTransportError);
    assert.deepEqual(failure.cause, { code: 23, signal: null, stderrPresent: true });
    assert.ok(!failure.message.includes("synthetic-private-stderr-canary"));
  }, { onStderr: undefined });
});

test("H266 parse and stderr callback failures keep the original cause, not a kill status", options, async () => {
  await withResult("malformed", "nonzero-malformed", async ({ iterator }) => {
    const result = await collect(iterator);
    assert.ok(result.failure instanceof HarnessTransportError);
    assert.ok(result.failure.cause instanceof SyntaxError);
  });
  const cause = new Error("synthetic stderr callback failure");
  await withResult("partial", "nonzero-callback", async ({ iterator, file, release }) => {
    const consuming = collect(iterator); await record(file("ready.json")); await release();
    assert.equal((await consuming).failure.cause, cause);
  }, { onStderr: () => { throw cause; } });
});

test("H266 a failed spawn remains a typed ENOENT, not a missing or zero exit result", options, async () => {
  const agent = new DeepAgent({ transport: "cli-jsonl", command: path.join(os.tmpdir(), "h266-no-binary", "missing.exe") });
  try {
    const result = await collect(agent.runJsonl("synthetic"));
    assert.ok(result.failure instanceof HarnessTransportError);
    assert.equal(result.failure.cause.code, "ENOENT");
  } finally { await agent.close(); }
});
