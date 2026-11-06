import assert from "node:assert/strict";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent } from "../dist/index.js";

const fixture = fileURLToPath(new URL("./approval-isolation-fixture.mjs", import.meta.url));

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

async function observe(mode) {
  const releaseApproval = deferred();
  const enteredApproval = deferred();
  const startedB = deferred();
  const approvals = [];
  const events = [];
  const agent = new DeepAgent({
    command: process.execPath,
    args: [fixture],
    onStderr: (line) => {
      if (line === "audit:started:thread-2") startedB.resolve();
    },
    onEvent: (event) => events.push(event),
    onApproval: async (request) => {
      approvals.push(request);
      enteredApproval.resolve();
      await releaseApproval.promise;
      return false;
    }
  });
  let iterator;
  let completion;
  try {
    const threadA = await agent.startThread();
    const threadB = await agent.startThread();
    assert.notEqual(threadA.id, threadB.id);
    const turnA = await threadA.run("A waits for a synthetic approval");
    if (mode !== "none") {
      await agent.requestForTest("audit/approval", { threadId: threadA.id });
      await enteredApproval.promise;
      assert.equal(approvals.length, 1);
      assert.equal(approvals[0].threadId, threadA.id);
      assert.equal(approvals[0].turnId, turnA.id);
      if (mode === "answered") {
        releaseApproval.resolve();
        await agent.waitForPendingApprovals();
      }
    }
    iterator = threadB.runStreamed("B has no approval and completes")[Symbol.asyncIterator]();
    const firstNext = iterator.next();
    await startedB.promise;
    // An RPC after turn/start lets its promise continuation establish turnId.
    // The terminal is emitted only after this barrier, excluding H-214.
    await agent.requestForTest("audit/barrier", {});
    await agent.requestForTest("audit/finish", { threadId: threadB.id });
    const first = await firstNext;
    assert.equal(first.done, false);
    assert.equal(first.value.type, "turn.completed");
    assert.equal(first.value.threadId, threadB.id);
    assert.notEqual(first.value.turnId, turnA.id);
    let finished = false;
    completion = iterator.next().then((value) => { finished = value.done; return value; });
    // Another real pipe round trip drains local promise continuations; no
    // deadline or elapsed-time guess is used to classify the iterator.
    const before = await agent.requestForTest("audit/barrier", {});
    const finishedBeforeRelease = finished;
    assert.equal(before.turns.find((turn) => turn.threadId === threadB.id).state, "completed");
    if (mode === "waiting") {
      assert.equal(before.turns.find((turn) => turn.threadId === threadA.id).state, "waiting_approval");
      assert.equal(before.approvalResponses, 0);
    }
    releaseApproval.resolve();
    await agent.waitForPendingApprovals();
    const end = await completion;
    assert.equal(end.done, true);
    const after = await agent.requestForTest("audit/barrier", {});
    assert.equal(after.approvalResponses, mode === "none" ? 0 : 1);
    assert.equal(events.filter((event) => event.type === "turn.completed" && event.threadId === threadB.id).length, 1);
    const result = { mode, finishedBeforeRelease, finishedAfterRelease: end.done,
      approvalsBefore: before.approvalResponses, approvalsAfter: after.approvalResponses,
      terminalB: first.value.type, threadA: threadA.id, threadB: threadB.id };
    console.log(JSON.stringify(result));
    return result;
  } finally {
    releaseApproval.resolve();
    await agent.waitForPendingApprovals();
    if (completion) await completion;
    if (iterator) await iterator.return();
    await agent.close();
  }
}

for (const mode of ["none", "answered"]) {
  test(`an independent completed stream finishes with ${mode} approval work`, { timeout: 10000 }, async () => {
    const result = await observe(mode);
    assert.equal(result.finishedBeforeRelease, true);
  });
}

test("a completed thread must not wait for another thread's unanswered approval", {
  timeout: 10000
}, async () => {
  const result = await observe("waiting");
  assert.equal(result.finishedBeforeRelease, true,
    "thread B already delivered its terminal, but unrelated thread A keeps its iterator open");
});
