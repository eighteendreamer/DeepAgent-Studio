import { createInterface } from "node:readline";
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
if (process.argv[2] === "jsonl") {
  send({ type: "turn.completed", threadId: "t", turnId: "r", message: "synthetic",
    ...(process.argv[3] === "legacy" ? {} : { protocolVersion: JSON.parse(process.argv[3]) }) });
} else createInterface({ input: process.stdin }).on("line", line => {
  const request = JSON.parse(line);
  if (request.method === "initialize") {
    send({ jsonrpc: "2.0", id: request.id, result: {
      protocolVersion: 1, serverName: "version-test", serverVersion: "1",
      capabilities: { approval: false, interrupt: false, reconnect: true, steer: false, streaming: true, threadLifecycle: true }
    } });
  } else if (request.method === "audit/emit") {
    send({ jsonrpc: "2.0", method: "harness/event", params: request.params });
    send({ jsonrpc: "2.0", id: request.id, result: {} });
  }
});
