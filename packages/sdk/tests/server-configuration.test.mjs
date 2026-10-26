// Explicit production-binary test, run after cargo build -p deepagent-cli.
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync } from "node:fs";
import { rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { DeepAgent } from "../dist/index.js";

const binary = process.env.DEEPAGENT_TEST_BINARY;
test("production stdio preserves explicit configuration across restart and cold fork", {
  skip: !binary,
  timeout: 20000
}, async () => {
  const root = mkdtempSync(path.join(tmpdir(), "deepagent-h04-sdk-"));
  const workspace = path.join(root, "workspace");
  mkdirSync(workspace);
  const previousRoot = process.env.DEEPAGENT_DATA_ROOT;
  const previousDb = process.env.DEEPAGENT_DB_PATH;
  process.env.DEEPAGENT_DATA_ROOT = path.join(root, "data");
  delete process.env.DEEPAGENT_DB_PATH;
  const agent = new DeepAgent({ command: binary, args: ["server", "--transport", "stdio"], cwd: workspace });
  try {
    const thread = await agent.startThread({ cwd: workspace, provider: "deepseek-official",
      model: "deepseek-flash", permissionProfile: "read_only", sandboxBackend: "direct" });
    const expected = { version: 1, overrides: { provider: "deepseek-official", model: "deepseek-flash",
      permission_profile: "read_only", sandbox_backend: "direct" } };
    assert.deepEqual(thread.configuration, expected);
    await agent.reconnect();
    // Source is not loaded in this new server process. Do not hide H-216 by
    // assuming fork loaded it: explicitly attach the child before inspecting.
    const fork = await agent.requestForTest("thread/fork", { threadId: thread.id, atSequence: 0 });
    assert.notEqual(fork.threadId, thread.id);
    assert.deepEqual((await agent.resumeThread(fork.threadId)).configuration, expected);
    assert.deepEqual((await agent.resumeThread(thread.id)).configuration, expected);
  } finally {
    await agent.close();
    if (previousRoot === undefined) delete process.env.DEEPAGENT_DATA_ROOT;
    else process.env.DEEPAGENT_DATA_ROOT = previousRoot;
    if (previousDb === undefined) delete process.env.DEEPAGENT_DB_PATH;
    else process.env.DEEPAGENT_DB_PATH = previousDb;
    // Only the freshly allocated test directory may be removed recursively.
    const resolved = path.resolve(root);
    assert.equal(path.dirname(resolved), path.resolve(tmpdir()));
    assert.ok(path.basename(resolved).startsWith("deepagent-h04-sdk-"));
    // SDK close sends kill but does not await process exit. Bounded filesystem
    // backoff handles Windows handle release; this is not a lifecycle assertion.
    await rm(resolved, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  }
});
