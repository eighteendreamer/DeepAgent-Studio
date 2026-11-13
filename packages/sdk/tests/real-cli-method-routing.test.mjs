// H-217: real server errors pass through the existing TypeScript SDK, no models.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { DeepAgent, HarnessRpcError } from "../dist/index.js";

test("H217 real CLI distinguishes unknown methods from invalid params without retiring SDK", { timeout: 20000 }, async () => {
  const binary = process.env.DEEPAGENT_TEST_BINARY ?? fileURLToPath(new URL(
    `../../../target/debug/deepagent${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
  assert.ok(binary && existsSync(binary), "provide the freshly built CLI; never skip this gate");
  const root = mkdtempSync(path.join(os.tmpdir(), "h217-native-"));
  for (const key of Object.keys(process.env)) {
    if (/API.?KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|PROXY|DEEPAGENT|DEEPSEEK|OPENAI|ANTHROPIC/iu.test(key)) delete process.env[key];
  }
  for (const key of ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "DEEPAGENT_DATA_ROOT"]) {
    process.env[key] = path.join(root, key.toLowerCase());
    mkdirSync(process.env[key]);
  }
  const workspace = path.join(root, "workspace");
  mkdirSync(workspace);
  const agent = new DeepAgent({ command: binary, cwd: workspace });
  try {
    for (const method of ["unknown", "thread/unknown", "rpc.extension", "Thread/list", ""]) {
      await assert.rejects(agent.requestForTest(method, { synthetic: "not-a-secret" }), (error) => {
        assert.ok(error instanceof HarnessRpcError);
        assert.equal(error.code, -32601);
        assert.deepEqual(error.data, { method });
        assert.ok(!error.message.includes("not-a-secret"));
        return true;
      });
    }
    for (const [method, params] of [["initialize", true], ["thread/start", { cwd: 42 }]]) {
      await assert.rejects(agent.requestForTest(method, params), (error) => error instanceof HarnessRpcError && error.code === -32602);
    }
    assert.deepEqual(await agent.listThreads(), [], "rejected routes never create sessions or force reconnect");
    console.log(JSON.stringify({ root, unknownCodes: 5, invalidParamCodes: 2, normalControl: true }));
  } finally {
    await agent.close();
  }
});
