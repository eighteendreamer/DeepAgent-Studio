import { createInterface } from "node:readline";

const threads = new Set();
let turnNumber = 0;

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function response(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function error(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

function event(params) {
  send({ jsonrpc: "2.0", method: "harness/event", params });
}

const input = createInterface({ input: process.stdin });
input.on("line", (line) => {
  const request = JSON.parse(line);
  switch (request.method) {
    case "initialize":
      response(request.id, {
        protocolVersion: 1,
        serverName: "mock-deepagent",
        serverVersion: "test",
        capabilities: {
          approval: true,
          interrupt: true,
          reconnect: true,
          steer: true,
          streaming: true,
          threadLifecycle: true
        }
      });
      break;
    case "thread/start": {
      const threadId = "thread-mock";
      threads.add(threadId);
      response(request.id, { threadId, status: "ready", cwd: request.params.cwd ?? null });
      event({
        type: "thread.started",
        threadId,
        title: null,
        protocolVersion: 1
      });
      break;
    }
    case "thread/resume":
      if (!threads.has(request.params.threadId)) {
        threads.add(request.params.threadId);
      }
      response(request.id, {
        threadId: request.params.threadId,
        status: "ready",
        cwd: null
      });
      break;
    case "turn/start": {
      const turnId = `turn-mock-${++turnNumber}`;
      response(request.id, {
        threadId: request.params.threadId,
        turnId,
        status: "started"
      });
      setTimeout(() => {
        event({
          type: "item.updated",
          threadId: request.params.threadId,
          turnId,
          itemId: null,
          item: { kind: "content_delta", text: "hello from mock" }
        });
        event({
          type: "approval.requested",
          approvalId: "approval-mock",
          threadId: request.params.threadId,
          turnId,
          toolName: "bash",
          reason: "test approval",
          scope: "tool"
        });
      }, 5);
      setTimeout(() => {
        event({
          type: "turn.completed",
          threadId: request.params.threadId,
          turnId,
          message: "completed"
        });
      }, 15);
      break;
    }
    case "approval/respond":
      response(request.id, {
        approvalId: request.params.approvalId,
        status: "resolved",
        approved: request.params.approved
      });
      break;
    case "turn/interrupt":
      response(request.id, {
        threadId: request.params.threadId,
        turnId: request.params.turnId,
        status: "cancelling"
      });
      break;
    case "turn/steer":
      response(request.id, {
        threadId: request.params.threadId,
        turnId: "turn-steered",
        replacesTurnId: request.params.turnId,
        status: "started"
      });
      break;
    case "thread/list":
      response(request.id, { threads: [] });
      break;
    default:
      error(request.id, -32601, `unknown method ${request.method}`);
  }
});
