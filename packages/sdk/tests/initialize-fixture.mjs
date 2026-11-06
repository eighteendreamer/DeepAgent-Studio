// Only synthetic stdio. Late duplicate success must not change readiness.
import { createInterface } from "node:readline";
const result = JSON.parse(process.argv[2]);
const mode = process.argv[3];
const valid = { protocolVersion: 1, serverName: "synthetic-peer", serverVersion: "1",
  capabilities: { threadLifecycle: true, streaming: true, interrupt: true,
    steer: true, approval: true, reconnect: true } };
const send = (value) => process.stdout.write(JSON.stringify(value) + "\n");
const reply = (id, result) => send({ jsonrpc: "2.0", id, result });
let business = 0;
createInterface({ input: process.stdin }).on("line", (line) => {
  const request = JSON.parse(line);
  if (request.method === "initialize") {
    if (request.params.protocolVersion !== 1) process.exit(31);
    if (mode === "rpc-error") send({ jsonrpc: "2.0", id: request.id, error: { code: -32602, message: "synthetic rejection" } });
    else reply(request.id, result);
    reply(request.id, valid);
    send({ jsonrpc: "2.0", method: "harness/event", params: { type: "thread.started", threadId: "handshake-barrier", protocolVersion: 1 } });
  } else if (request.method === "audit/count") reply(request.id, { business });
  else { business++; reply(request.id, { threadId: "t", turnId: "r", threads: [] }); }
});
