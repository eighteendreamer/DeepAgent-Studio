import assert from "node:assert/strict";
import { ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import test, { mock } from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./jsonl-owner-fixture.mjs", import.meta.url));
const options = { timeout: 20000 };
const alive = (pid) => {
  try { process.kill(pid, 0); return true; }
  catch (error) { if (error.code === "ESRCH") return false; throw error; }
};
async function bounded(task, label) {
  let timer;
  try {
    return await Promise.race([task, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} did not settle`)), 2500);
    })]);
  } finally { clearTimeout(timer); }
}
async function record(file, limit = 2500) {
  const deadline = Date.now() + limit;
  while (Date.now() < deadline) {
    try { return JSON.parse(await readFile(file, "utf8")); }
    catch (error) {
      if (error.code !== "ENOENT" && !(error instanceof SyntaxError)) throw error;
    }
    await delay(10);
  }
  throw new Error("test-owned process did not acknowledge its gate");
}
async function withOwner(mode, body, onStderr) {
  const prefix = path.join(os.tmpdir(), "deepagent-h267-owner-");
  const root = await mkdtemp(prefix);
  const agent = new DeepAgent({ transport: "cli-jsonl", command: process.execPath, args: [fixture, root, mode], onStderr });
  const ids = new Set();
  const iterators = [];
  const file = (id, suffix) => path.join(root, `${id}.${suffix}`);
  const open = (id, extra) => {
    ids.add(id);
    const iterator = agent.runJsonl(id, extra)[Symbol.asyncIterator]();
    iterators.push(iterator);
    return iterator;
  };
  const pid = async (id) => (await record(file(id, "ready.json"))).pid;
  try { await body({ agent, open, pid, file, root }); }
  finally {
    for (const id of ids) await writeFile(file(id, "release"), "release synthetic fixture");
    for (const iterator of iterators) await iterator.return?.().catch(() => {});
    await agent.close().catch(() => {});
    for (const id of ids) {
      if (!existsSync(file(id, "ready.json"))) continue; // An unused iterator must never spawn.
      const owned = [await pid(id)];
      if (existsSync(file(id, "worker-ready.json"))) owned.push((await record(file(id, "worker-ready.json"))).pid);
      for (const processId of owned) {
        const deadline = Date.now() + 9000;
        while (alive(processId) && Date.now() < deadline) await delay(10);
        assert.equal(alive(processId), false, "only remove the fixture after its process exits");
      }
    }
    assert.ok(root.startsWith(prefix));
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 25 });
  }
}

test("H267 return preempts a parked next and waits for the actual child exit", options, async () => {
  await withOwner("normal", async ({ open, pid }) => {
    const iterator = open("parked-return");
    assert.equal((await iterator.next()).done, false);
    const processId = await pid("parked-return");
    const pending = iterator.next();
    assert.equal((await bounded(iterator.return(), "return during next")).done, true);
    assert.equal((await bounded(pending, "parked next")).done, true);
    assert.equal(alive(processId), false);
  });
});

test("H267 explicit close wakes a silent read, is idempotent, and prevents restart", options, async () => {
  await withOwner("silent", async ({ agent, open, pid, file }) => {
    const iterator = open("silent");
    const pending = iterator.next();
    const processId = await pid("silent");
    await bounded(Promise.all([agent.close(), agent.close()]), "concurrent close");
    assert.equal(alive(processId), false);
    assert.equal((await bounded(pending, "silent next")).done, true);
    const unused = open("after-close");
    await assert.rejects(unused.next(), HarnessTransportError);
    assert.equal(existsSync(file("after-close", "ready.json")), false);
  });
});

test("H267 consumer throw retains its error and reaps the process while next is parked", options, async () => {
  await withOwner("normal", async ({ open, pid }) => {
    const iterator = open("consumer-throw");
    await iterator.next();
    const processId = await pid("consumer-throw");
    const pending = iterator.next();
    const failure = new Error("synthetic consumer failure");
    await assert.rejects(bounded(iterator.throw(failure), "throw during next"), (error) => error === failure);
    await bounded(pending, "next before throw");
    assert.equal(alive(processId), false);
  });
});

test("H267 ending one iterator leaves another alive until shared close owns both", options, async () => {
  await withOwner("normal", async ({ agent, open, pid, file }) => {
    const first = open("first");
    const second = open("second");
    await Promise.all([first.next(), second.next()]);
    const [firstPid, secondPid] = await Promise.all([pid("first"), pid("second")]);
    await bounded(first.return(), "first return");
    assert.equal(alive(firstPid), false);
    assert.equal(alive(secondPid), true);
    await writeFile(file("second", "probe"), "live independent run");
    assert.equal((await record(file("second", "ack.json"))).pid, secondPid);
    await bounded(agent.close(), "shared close");
    assert.equal(alive(secondPid), false);
  });
});

test("H267 unused iterator return is lazy, including after owner close", options, async () => {
  await withOwner("normal", async ({ agent, open, file }) => {
    assert.equal((await open("unused").return()).done, true);
    await agent.close();
    assert.equal((await open("closed-unused").return()).done, true);
    for (const id of ["unused", "closed-unused"]) assert.equal(existsSync(file(id, "ready.json")), false);
  });
});

test("H267 a terminal event does not discard the CLI post-terminal work before explicit close", options, async () => {
  await withOwner("terminal-wait", async ({ agent, open, pid, file }) => {
    const iterator = open("post-terminal");
    assert.equal((await iterator.next()).value.type, "turn.completed");
    const processId = await pid("post-terminal");
    await writeFile(file("post-terminal", "probe"), "post-terminal drain is still owned");
    assert.equal((await record(file("post-terminal", "ack.json"))).pid, processId);
    await bounded(agent.close(), "post-terminal close");
    assert.equal(alive(processId), false);
  });
});

test("H267 abort before start spawns nothing; active abort releases a parked consumer", options, async () => {
  await withOwner("silent", async ({ open, pid, file }) => {
    const before = new AbortController();
    before.abort();
    await assert.rejects(open("pre-aborted", { signal: before.signal }).next(), HarnessTransportError);
    assert.equal(existsSync(file("pre-aborted", "ready.json")), false);
    const active = new AbortController();
    const iterator = open("active-abort", { signal: active.signal });
    const pending = iterator.next();
    // Attach a rejection observer before abort so it cannot become unhandled.
    const rejected = assert.rejects(pending, HarnessTransportError);
    const processId = await pid("active-abort");
    active.abort();
    await bounded(rejected, "aborted read");
    assert.equal(alive(processId), false);
    assert.equal((await iterator.return()).done, true);
  });
});

test("H267 spawn failure is stable and owner close has no dangling work", options, async () => {
  const agent = new DeepAgent({ transport: "cli-jsonl", command: path.join(os.tmpdir(), "h267-nonexistent-binary", "never.exe") });
  try {
    await assert.rejects(bounded(agent.runJsonl("spawn-failure")[Symbol.asyncIterator]().next(), "spawn failure"), HarnessTransportError);
  } finally { await bounded(agent.close(), "failed spawn close"); }
});

for (const refusal of ["false", "throw"]) {
  test(`H267 ${refusal} termination cannot report close success or forget the live owner`, options, async () => {
    await withOwner("normal", async ({ agent, open, pid }) => {
      const iterator = open("termination-failure");
      await iterator.next();
      const processId = await pid("termination-failure");
      const original = ChildProcess.prototype.kill;
      const failure = new Error("synthetic native termination refusal");
      const intercept = mock.method(ChildProcess.prototype, "kill", function (signal) {
        if (this.pid !== processId) return original.call(this, signal);
        if (refusal === "throw") throw failure;
        return false;
      });
      try {
        await assert.rejects(bounded(agent.close(), "refused close"), HarnessTransportError);
        assert.equal(alive(processId), true, "a refused kill must remain tracked, not called reaped");
      } finally { intercept.mock.restore(); }
      await bounded(agent.close(), "explicit retry after restored native API");
      assert.equal(alive(processId), false);
    });
  });
}

test("H267 normal completion drains trailing stderr without terminating post-terminal work", options, async () => {
  const messages = [];
  await withOwner("stderr", async ({ open, pid, file }) => {
    const iterator = open("stderr-drain");
    await iterator.next();
    const processId = await pid("stderr-drain");
    await writeFile(file("stderr-drain", "release"), "complete normally");
    const rest = [];
    for await (const event of iterator) rest.push(event.type);
    assert.deepEqual(rest, ["turn.completed"]);
    assert.equal(alive(processId), false);
    assert.deepEqual(messages, Array.from({ length: 1000 }, (_, i) => `synthetic stderr ${i}`));
  }, (line) => messages.push(line));
});

test("H267 a stubborn direct child is reaped, not merely marked killed", options, async () => {
  await withOwner("ignore-term", async ({ open, pid }) => {
    const iterator = open("stubborn");
    await iterator.next();
    const processId = await pid("stubborn");
    await bounded(iterator.return(), "stubborn reap");
    assert.equal(alive(processId), false);
  });
});

test("H267 stderr callback failure retains its cause and still reaps its direct child", options, async () => {
  const failure = new Error("synthetic stderr consumer failure");
  await withOwner("stderr", async ({ open, pid, file }) => {
    const iterator = open("stderr-failure");
    await iterator.next();
    const processId = await pid("stderr-failure");
    const consumed = (async () => { for await (const _event of iterator) { /* drain */ } })();
    const rejected = assert.rejects(consumed, (error) => error instanceof HarnessTransportError && error.cause === failure);
    await writeFile(file("stderr-failure", "release"), "emit synthetic stderr");
    await bounded(rejected, "stderr failure cleanup");
    assert.equal(alive(processId), false);
  }, () => { throw failure; });
});

test("H267 direct owner cleanup does not claim arbitrary wrapper descendant ownership", options, async () => {
  await withOwner("tree", async ({ open, pid, file }) => {
    const iterator = open("tree-return");
    await iterator.next();
    const root = await pid("tree-return");
    const descendant = (await record(file("tree-return", "worker-ready.json"))).pid;
    assert.equal(alive(descendant), true, "descendant must run before the root is stopped");
    await bounded(iterator.return(), "tree root return");
    assert.equal(alive(root), false);
    assert.equal(alive(descendant), true, "custom wrapper descendants need their own native process owner");
    await writeFile(file("tree-return", "probe"), "scope observation after root return");
    assert.equal((await record(file("tree-return", "worker-ack.json"))).pid, descendant);
    // The test's own release gate, not SDK kill, reclaims this deliberate out-of-scope descendant.
  });
});

test("H267 root exit with inherited descendant pipes rejects instead of hanging or claiming completion", options, async () => {
  await withOwner("tree", async ({ open, pid, file }) => {
    const iterator = open("tree-eof");
    await iterator.next();
    const root = await pid("tree-eof");
    const descendant = (await record(file("tree-eof", "worker-ready.json"))).pid;
    assert.equal(alive(descendant), true, "descendant must run before the root exits");
    // The descendant deliberately keeps both inherited pipes open after root exit.
    await writeFile(file("tree-eof", "root-exit"), "exit only the root");
    await assert.rejects(bounded(iterator.next(), "inherited pipe EOF"), /descendant ownership is unresolved/u);
    assert.equal(alive(root), false);
    assert.equal(alive(descendant), true);
  });
});
