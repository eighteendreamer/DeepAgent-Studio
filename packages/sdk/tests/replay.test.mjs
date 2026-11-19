import test from "node:test";
import assert from "node:assert/strict";
import { watchThreadEvents } from "../dist/replay.js";

const page = (session = [], run = [], more = false, cursor = null) => ({
  threadId: "t", historyPageVersion: 1, cursorMode: "stream_scoped", hasMore: more, nextCursor: cursor,
  events: session.map(streamSequence => ({ stream: "session", streamSequence })),
  runEvents: [{ runId: "r", events: run.map(streamSequence => ({ stream: "run", runId: "r", streamSequence })) }]
});

test("durable follow pages, waits for new rows and resumes independent streams without duplicates", async () => {
  const calls = [];
  const pages = [page([0], [0], true, "next"), page([1], [1]), page([], []), page([2], [2])];
  const agent = { async readThreadPage(id, options) { calls.push([id, structuredClone(options)]); return pages.shift(); } };
  const stream = watchThreadEvents(agent, "t");
  assert.equal((await stream.next()).value.events.length, 2);
  const second = (await stream.next()).value;
  assert.deepEqual(calls[1][1], { cursor: "next" });
  const third = (await stream.next()).value;
  assert.equal(third.events.length, 2);
  assert.deepEqual(calls[2][1], { sessionAfterSequence: 1, runAfterSequences: { r: 1 } });
  await stream.return();
  const restored = JSON.parse(JSON.stringify(second.checkpoint));
  const resumed = watchThreadEvents({ async readThreadPage() { return page([0, 1, 2], [0, 1, 2]); } }, "t", { checkpoint: restored });
  assert.deepEqual((await resumed.next()).value.events.map(e => e.sequence), [2, 2]);
  await resumed.return();
  assert.deepEqual(restored, second.checkpoint);
});

test("replay rejects invalid cursor, order and unsafe sequence without mutating caller checkpoint", async () => {
  for (const bad of [page([2, 1]), page([Number.MAX_SAFE_INTEGER + 1]), page([], [], true)]) {
    const checkpoint = { sessionAfterSequence: 0, runAfterSequences: {} };
    const stream = watchThreadEvents({ async readThreadPage() { return bad; } }, "t", { checkpoint });
    await assert.rejects(stream.next());
    assert.deepEqual(checkpoint, { sessionAfterSequence: 0, runAfterSequences: {} });
  }
});

test("abort and read failures never admit a turn, ACK, retry or reconnect", async () => {
  let calls = 0;
  const controller = new AbortController(); controller.abort();
  const stream = watchThreadEvents({ async readThreadPage() { calls++; } }, "t", { signal: controller.signal });
  await assert.rejects(stream.next()); assert.equal(calls, 0);
  const failure = new Error("synthetic read failure");
  const failed = watchThreadEvents({ async readThreadPage() { calls++; throw failure; } }, "t");
  await assert.rejects(failed.next(), error => error === failure); assert.equal(calls, 1);
});

test("replay binds checkpoints and durable rows to their thread and stream", async () => {
  let calls = 0;
  const foreign = watchThreadEvents({ async readThreadPage() { calls++; } }, "t", {
    checkpoint: { threadId: "other", runAfterSequences: {} }
  });
  await assert.rejects(foreign.next(), /another thread/); assert.equal(calls, 0);
  for (const field of ["stream", "runId"]) {
    const bad = page([], [1]); bad.runEvents[0].events[0][field] = "other";
    const stream = watchThreadEvents({ async readThreadPage() { return bad; } }, "t");
    await assert.rejects(stream.next(), /another stream/);
  }
});
