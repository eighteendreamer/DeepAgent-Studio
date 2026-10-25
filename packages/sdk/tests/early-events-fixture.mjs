import { createInterface } from "node:readline";

const mode = process.argv[2];
const threadId = "early-thread";
const turnId = "early-turn";
const response = (id, result) => ({ jsonrpc: "2.0", id, result });
const notification = (event) => ({ jsonrpc: "2.0", method: "harness/event", params: event });
const delta = { type: "item.updated", threadId, turnId,
  item: { kind: "content_delta", text: "early canary" } };
const completed = { type: "turn.completed", threadId, turnId, message: "original terminal" };
const send = (messages) => process.stdout.write(messages.map(JSON.stringify).join("\n") + "\n");
const watchdog = setTimeout(() => process.exit(24), 6000);
let started = false;

createInterface({ input: process.stdin }).on("line", (line) => {
  const request = JSON.parse(line);
  switch (request.method) {
    case "initialize":
      send([response(request.id, { protocolVersion: 1, serverName: "early-events-audit",
        serverVersion: "1", capabilities: {} })]);
      break;
    case "thread/start":
      send([response(request.id, { threadId, status: "ready" })]);
      break;
    case "turn/start": {
      started = true;
      const reply = response(request.id, { threadId, turnId, status: "started" });
      if (mode === "event-first") send([notification(delta), notification(completed), reply]);
      else if (mode === "response-first") send([reply, notification(delta), notification(completed)]);
      else if (mode === "controlled") send([reply]);
      else throw new Error(`unknown mode ${mode}`);
      process.stderr.write("audit: turn/start processed\n");
      break;
    }
    case "audit/barrier":
      send([response(request.id, { started })]);
      break;
    case "audit/release":
      // In the control, the parent releases only after turn/start resumed.
      // In fault groups this late terminal frees the parked high-level stream.
      send([response(request.id, {}),
        ...(mode === "controlled" ? [notification(delta), notification(completed)] :
          [notification({ ...completed, message: "cleanup terminal" })])]);
      break;
    case "audit/exit":
      clearTimeout(watchdog);
      process.exit(0);
      break;
    default:
      throw new Error(`unexpected method ${request.method}`);
  }
});
