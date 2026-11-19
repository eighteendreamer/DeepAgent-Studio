import test from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { DeepAgent } from "../dist/index.js";

for (const protocolVersion of [2, null, "1"]) test(`stdio rejects explicit incompatible event version ${JSON.stringify(protocolVersion)} after v1/legacy`, async () => {
  const agent = new DeepAgent({ command: process.execPath, args: [fileURLToPath(new URL("./event-version-fixture.mjs", import.meta.url))] });
  const seen = [];
  agent.subscribe(event => seen.push(event));
  try {
    await agent.requestForTest("audit/emit", { type: "turn.started", threadId: "t", turnId: "r", protocolVersion: 1 });
    await agent.requestForTest("audit/emit", { type: "item.updated", threadId: "t", turnId: "r", item: { kind: "content_delta", text: "legacy" } });
    assert.equal(seen.length, 2);
    await assert.rejects(agent.requestForTest("audit/emit", { type: "turn.completed", threadId: "t", turnId: "r", message: "synthetic", protocolVersion }));
    assert.equal(seen.length, 2);
  } finally { await agent.close(); }
});

test("CLI JSONL uses the same event version contract as stdio", async () => {
  for (const version of ["legacy", "1", "2", "null", '"1"']) {
    const agent = new DeepAgent({ transport: "cli-jsonl", command: process.execPath,
      args: [fileURLToPath(new URL("./event-version-fixture.mjs", import.meta.url)), "jsonl", version] });
    const events = [];
    const consume = async () => { for await (const event of agent.runJsonl("synthetic")) events.push(event); };
    try {
      if (version === "legacy" || version === "1") {
        await consume(); assert.equal(events.length, 1);
      } else {
        await assert.rejects(consume(), /incompatible event protocol version/); assert.equal(events.length, 0);
      }
    } finally { await agent.close(); }
  }
});
