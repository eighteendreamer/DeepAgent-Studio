// Explicit, offline integration gate. The optional node:sqlite test helper is
// not a production dependency; this gate requires Node >=22.13 and a real CLI.
import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import test, { mock } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessTransportError } from "../dist/index.js";

test("H266/H267 real CLI exit results, owned drain and durable reopen do not redispatch or invent cancellation", { timeout: 30000 }, async () => {
  const source = process.env.DEEPAGENT_TEST_BINARY ?? fileURLToPath(new URL(`../../../target/debug/deepagent${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  assert.ok(existsSync(source), "build the current real CLI first; no skipped fallback");
  const root = mkdtempSync(path.join(os.tmpdir(), "deepagent-h267-real-cli-"));
  const binary = path.join(root, path.basename(source));
  copyFileSync(source, binary);
  const hash = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");
  assert.equal(hash(binary), hash(source));
  for (const name of Object.keys(process.env)) {
    if (/(API.?KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|DEEPAGENT|DEEPSEEK|OPENAI|ANTHROPIC)/iu.test(name)) delete process.env[name];
  }
  for (const name of ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME"]) {
    process.env[name] = path.join(root, name.toLowerCase());
    mkdirSync(process.env[name], { recursive: true });
  }
  process.env.DEEPAGENT_DATA_ROOT = path.join(root, "data");
  const workspace = path.join(root, "workspace");
  mkdirSync(workspace);
  const launched = [];
  const original = childProcess.spawn;
  // Observe the actual native ChildProcess, preserving every spawn option and
  // byte. No fake process, provider, exit notification or JSONL is substituted.
  const observation = mock.method(childProcess, "spawn", function (...args) {
    const child = original.apply(this, args);
    if (args[0] === binary) launched.push({ child, closed: once(child, "close") });
    return child;
  });
  syncBuiltinESMExports();
  const clients = [];
  const agent = (extra = {}) => {
    const value = new DeepAgent({ transport: "cli-jsonl", command: binary, cwd: workspace, ...extra });
    clients.push(value);
    return value;
  };
  const database = path.join(process.env.DEEPAGENT_DATA_ROOT, "deepagent.db");
  const snapshot = () => {
    const db = new DatabaseSync(database, { readOnly: true });
    try {
      assert.equal(db.prepare("PRAGMA integrity_check").get().integrity_check, "ok");
      return {
        sessions: db.prepare("SELECT id FROM sessions ORDER BY id").all(),
        runs: db.prepare("SELECT id, session_id, state, terminal_kind FROM runs ORDER BY id").all(),
        terminals: db.prepare("SELECT run_id, sequence, data FROM run_events WHERE event_type='run_terminal' ORDER BY run_id, sequence").all()
      };
    } finally { db.close(); }
  };
  const failedProcess = async (client, prompt, code) => {
    const events = [];
    await assert.rejects(async () => {
      for await (const event of client.runJsonl(prompt)) events.push(event);
    }, (error) => {
      assert.ok(error instanceof HarnessTransportError);
      assert.deepEqual(error.cause, { code, signal: null, stderrPresent: true });
      return true;
    });
    assert.deepEqual(await launched.at(-1).closed, [code, null]);
    await client.close();
    return events;
  };
  let lock;
  try {
    const complete = agent();
    const events = [];
    for await (const event of complete.runJsonl("/help")) events.push(event);
    await complete.close();
    const control = launched.at(-1);
    assert.deepEqual(await control.closed, [0, null]);
    assert.equal(events.filter((event) => event.type === "turn.completed").length, 1);
    assert.equal(events.filter((event) => event.type === "turn.interrupted").length, 0);
    const afterControl = snapshot();
    assert.equal(afterControl.runs.length, 1);
    assert.equal(afterControl.runs[0].terminal_kind, "succeeded");
    assert.equal(afterControl.terminals.length, 1);

    const failing = agent();
    const failedEvents = await failedProcess(failing,
      "Synthetic local input; no provider configuration is installed.", 1);
    // Business failure output and native process failure are distinct receipts.
    // Both must survive; neither creates a local cancellation or another run.
    assert.equal(failedEvents.filter((event) => event.type === "error" && event.code === "run_failed").length, 1);
    assert.equal(failedEvents.filter((event) => event.type === "turn.interrupted").length, 0);
    const before = snapshot();
    const newRuns = before.runs.filter((run) => !afterControl.runs.some((prior) => prior.id === run.id));
    assert.equal(newRuns.length, 1, "native CLI persists one failed run even when its wire result is error/run_failed");
    const { session_id: threadId, id: turnId } = newRuns[0];
    assert.equal(before.runs.filter((run) => run.id === turnId && run.terminal_kind === "failed").length, 1);
    assert.equal(before.terminals.filter((event) => event.run_id === turnId).length, 1);

    // Actual parser rejection before Runtime/DB initialization: no fake wrapper
    // or injected exit event, and no new persistent run to reconcile.
    assert.deepEqual(await failedProcess(agent({ args: ["run", "--h266-unknown-option"] }), "/help", 2), []);
    assert.deepEqual(snapshot(), before);
    // Actual startup failure before runtime callbacks, confined to a synthetic
    // file where a directory is required. Do not damage or migrate a real DB.
    const dataRoot = process.env.DEEPAGENT_DATA_ROOT;
    const notDirectory = path.join(root, "not-a-data-directory");
    writeFileSync(notDirectory, "synthetic startup denial");
    try {
      process.env.DEEPAGENT_DATA_ROOT = notDirectory;
      assert.deepEqual(await failedProcess(agent(), "/help", 1), []);
    } finally { process.env.DEEPAGENT_DATA_ROOT = dataRoot; }
    assert.equal(readFileSync(notDirectory, "utf8"), "synthetic startup denial");
    assert.deepEqual(snapshot(), before, "native early failures must not invent a run or cancellation");

    lock = new DatabaseSync(database);
    // Existing native Database::open writes migration/maintenance metadata.
    // Hold only this synthetic DB's writer slot, without changing its schema.
    lock.exec("BEGIN IMMEDIATE");
    const closing = agent();
    const iterator = closing.runJsonl("/help")[Symbol.asyncIterator]();
    const pending = iterator.next();
    const blocked = launched.at(-1);
    await once(blocked.child, "spawn");
    assert.equal(blocked.child.exitCode, null);
    assert.equal(await Promise.race([pending.then(() => "settled"), delay(100).then(() => "parked")]), "parked");
    await closing.close();
    const [exitCode, signal] = await blocked.closed;
    assert.equal((await pending).done, true);
    assert.ok(exitCode !== null || signal !== null, "native exit receipt precedes releasing the DB gate");
    assert.throws(() => process.kill(blocked.child.pid, 0), { code: "ESRCH" });
    lock.exec("ROLLBACK");
    lock.close();
    lock = undefined;
    assert.deepEqual(snapshot(), before, "startup close added no run, terminal or redispatch");

    const reopened = new DeepAgent({ command: binary, cwd: workspace });
    clients.push(reopened);
    await reopened.resumeThread(threadId);
    const history = await reopened.requestForTest("thread/read", { threadId });
    const run = history.runEvents.find((entry) => entry.runId === turnId);
    assert.equal(run.terminalKind, "failed");
    assert.equal(run.events.filter((event) => event.event_type === "run_terminal").length, 1);
    assert.deepEqual(run.actions, []);
    await reopened.close();
    assert.deepEqual(snapshot(), before, "reopening reconciles the original terminal without redispatch");
    console.log(JSON.stringify({ root, binarySha256: hash(binary), events: events.map((event) => event.type), failed: { threadId, turnId }, nativeProcesses: launched.length,
      nativeExitReceipts: await Promise.all(launched.map((entry) => entry.closed)) }));
  } finally {
    if (lock) { lock.exec("ROLLBACK"); lock.close(); }
    await Promise.allSettled(clients.map((client) => client.close()));
    observation.mock.restore();
    syncBuiltinESMExports();
    // Retain synthetic DB/transcript evidence, never remove before exit receipt.
    await Promise.all(launched.map((entry) => entry.closed));
  }
});
