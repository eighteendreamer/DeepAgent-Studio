// Finite stdio-only peer. All identifiers, content and readback are synthetic.
import { createInterface } from "node:readline";

const mode = process.argv[2];
const response = (id, result) => ({ jsonrpc: "2.0", id, result });
const notification = (event) => ({ jsonrpc: "2.0", method: "harness/event", params: event });
const send = (messages) => process.stdout.write(messages.map(JSON.stringify).join("\n") + "\n");
const threads = [];
const starts = [];
const withheld = [];
const watchdog = setTimeout(() => process.exit(24), 6000);
const terminal = (threadId, type = "turn.completed") => ({
  type, threadId, turnId: `turn-${threadId}`, message: "original terminal", reason: "synthetic stop"
});

createInterface({ input: process.stdin }).on("line", (line) => {
  const request = JSON.parse(line);
  switch (request.method) {
    case "initialize":
      send([response(request.id, { protocolVersion: 1, serverName: "owned-stream-fixture", serverVersion: "1" })]);
      break;
    case "thread/start": {
      const threadId = `thread-${threads.length + 1}`;
      threads.push(threadId);
      send([response(request.id, { threadId, status: "ready" })]);
      break;
    }
    case "turn/start": {
      const { threadId } = request.params;
      starts.push(request.params);
      const reply = response(request.id, { threadId, turnId: `turn-${threadId}`, status: "started" });
      if (mode === "rpc-error") {
        send([{ jsonrpc: "2.0", id: request.id, error: { code: -32005, message: "synthetic busy" } }]);
      } else if (mode === "hold-start") {
        withheld.push(reply);
      } else if (mode === "burst" || mode === "large") {
        const events = Array.from({ length: mode === "burst" ? 300 : 1 }, (_, index) => notification({
          type: "item.updated", threadId, turnId: `turn-${threadId}`,
          item: { kind: "content_delta", text: mode === "large" ? "x".repeat(4 * 1024 * 1024 + 1) : String(index) }
        }));
        send([...events, notification(terminal(threadId)), reply]);
      } else if (mode === "early-failed" || mode === "early-interrupted") {
        send([notification(terminal(threadId, mode === "early-failed" ? "turn.failed" : "turn.interrupted")), reply]);
      } else if (mode === "candidate-isolation") {
        send([
          notification(terminal("other-thread")),
          notification({ ...terminal(threadId), turnId: "other-turn" }),
          notification({ type: "item.updated", threadId, turnId: `turn-${threadId}`,
            item: { kind: "content_delta", text: "original" } }),
          notification({ ...terminal(threadId), threadId: undefined }),
          reply
        ]);
      } else if (mode === "invalid-thread" || mode === "invalid-turn") {
        if (mode === "invalid-thread") reply.result.threadId = "PRIVATE_BAD_ID";
        else reply.result.turnId = " \t";
        send([notification(terminal(threadId)), reply]);
      } else if (mode === "terminal-eof") {
        send([notification(terminal(threadId)), reply]);
        process.stdout.end();
      } else {
        send([reply]);
      }
      process.stderr.write(`audit:start:${threadId}\n`);
      break;
    }
    case "audit/barrier":
      send([response(request.id, { starts })]);
      break;
    case "audit/emit":
      send([...(request.params.events ?? []).map(notification), response(request.id, {})]);
      break;
    case "audit/release":
      send([...withheld.splice(0), ...starts.map(({ threadId }) => notification(terminal(threadId))), response(request.id, {})]);
      break;
    case "audit/exit":
      clearTimeout(watchdog);
      process.exit(0);
      break;
    default:
      throw new Error(`unexpected method ${request.method}`);
  }
});
