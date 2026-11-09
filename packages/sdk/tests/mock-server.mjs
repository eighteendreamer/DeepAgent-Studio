import { createInterface } from "node:readline";
import { readFileSync } from "node:fs";

const threads = new Set();
const configurations = new Map();
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
      // Only emit the new field when testing a configured thread. Unconfigured
      // responses remain the older-server compatibility control.
      const configuration = request.params.permissionProfile || request.params.reasoningEffort !== undefined ? {
        version: 1, overrides: {
          ...(request.params.permissionProfile ? {permission_profile: request.params.permissionProfile} : {}),
          ...(request.params.reasoningEffort !== undefined ? {reasoning_effort: request.params.reasoningEffort} : {})
        }
      } : undefined;
      configurations.set(threadId, configuration);
      response(request.id, { threadId, status: "ready", cwd: request.params.cwd ?? null, configuration });
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
        cwd: null,
        configuration: configurations.get(request.params.threadId) ?? null
      });
      break;
    case "thread/read": {
      if (request.params.threadId === "legacy-history") {
        response(request.id, { threadId: "legacy-history", events: [], runEvents: [] });
        break;
      }
      response(request.id, {
        threadId: request.params.threadId, title: null, cwd: null,
        controlProjectionVersion: 1, cursorMode: "stream_scoped", historyPageVersion: 1,
        events: [{ request: request.params }], runEvents: [],
        hasMore: !request.params.cursor, nextCursor: request.params.cursor ? null : "fixture-next"
      });
      break;
    }
    case "turn/start": {
      if (request.params.input === "fixture:busy") {
        error(request.id, -32005, "thread is busy; use turn/steer explicitly");
        break;
      }
      const turnId = `turn-mock-${++turnNumber}`;
      response(request.id, {
        threadId: request.params.threadId,
        turnId,
        status: "started",
        snapshotId: "snapshot-start-mock",
        capabilitySnapshot: { runId: turnId, threadId: request.params.threadId, snapshotId: "snapshot-start-mock" }
      });
      setTimeout(() => {
        if (request.params.input === "fixture:h127-tool-image") {
          const fixture = JSON.parse(readFileSync(new URL("../../../crates/deepagent-harness-protocol/tests/fixtures/tool-image-result-v1.json", import.meta.url), "utf8"));
          event({ ...fixture, threadId: request.params.threadId, turnId });
          return;
        }
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
        status: "started",
        snapshotId: "snapshot-steer-mock",
        capabilitySnapshot: { runId: "turn-steered", threadId: request.params.threadId, snapshotId: "snapshot-steer-mock" }
      });
      break;
    case "thread/list":
      response(request.id, { threads: [] });
      break;
    case "tool/list": {
      if (request.params.expectedSnapshotId && request.params.expectedSnapshotId !== "snapshot-mock") {
        error(request.id, -32602, "stale capability snapshot");
        break;
      }
      const result = { protocolVersion: 1, tools: [{ name: "read_file" }] };
      if (request.params.threadId) {
        result.threadId = request.params.threadId;
        result.runId = request.params.runId ?? "run-mock";
        result.snapshotId = "snapshot-mock";
        result.capabilitySnapshot = { snapshotId: result.snapshotId };
      }
      response(request.id, result);
      break;
    }
    case "config/read":
      response(request.id, {configSnapshotVersion: 1, configSnapshotId: "config-mock", request: request.params});
      break;
    default:
      error(request.id, -32601, `unknown method ${request.method}`);
  }
});
