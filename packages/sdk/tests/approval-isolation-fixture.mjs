import { createInterface } from "node:readline";

let nextThread = 0;
const turns = new Map();
let approvalResponses = 0;
const mode = process.argv[2] ?? "audit";
const responses = [];
const withheld = [];
const watchdog = setTimeout(() => process.exit(24), 8000);
const send = (message) => process.stdout.write(`${JSON.stringify(message)}\n`);
const reply = (id, result) => send({ jsonrpc: "2.0", id, result });
const event = (params) => send({ jsonrpc: "2.0", method: "harness/event", params });

createInterface({ input: process.stdin }).on("line", (line) => {
  const request = JSON.parse(line);
  switch (request.method) {
    case "initialize":
      reply(request.id, { protocolVersion: 1, serverName: "approval-isolation-audit",
        serverVersion: "1", capabilities: { threadLifecycle: true, streaming: true, interrupt: true, steer: true, approval: true, reconnect: true } });
      break;
    case "thread/start":
      reply(request.id, { threadId: `thread-${++nextThread}`, status: "ready" });
      break;
    case "turn/start": {
      const { threadId } = request.params;
      const turn = { threadId, turnId: `turn-${threadId}`, state: "running" };
      turns.set(threadId, turn);
      if (mode === "early") {
        event({ type: "approval.requested", threadId, turnId: turn.turnId,
          approvalId: "early-approval", reason: "synthetic early approval" });
        event({ type: "turn.completed", threadId, turnId: turn.turnId, message: "early terminal" });
      }
      reply(request.id, { threadId, turnId: turn.turnId, status: "started" });
      process.stderr.write(`audit:started:${threadId}\n`);
      break;
    }
    case "audit/approval": {
      const turn = turns.get(request.params.threadId);
      if (!turn) throw new Error("approval requires an existing turn");
      turn.state = "waiting_approval";
      event({ type: "approval.requested", threadId: turn.threadId, turnId: turn.turnId,
        approvalId: "approval-A", toolName: "audit_tool", reason: "synthetic approval", scope: "single_call" });
      reply(request.id, {});
      break;
    }
    case "approval/respond": {
      responses.push(request.params);
      if (mode !== "audit") {
        if (mode === "hold-receipt") withheld.push(request);
        else if (mode === "rpc-error") send({ jsonrpc: "2.0", id: request.id,
          error: { code: -32003, message: "synthetic approval no longer pending" } });
        else reply(request.id, { status: mode === "invalid-receipt" ? "invalid" : "resolved",
          approvalId: request.params.approvalId, approved: request.params.approved });
        break;
      }
      if (request.params.approvalId !== "approval-A") throw new Error("wrong approval identity");
      approvalResponses++;
      turns.get("thread-1").state = "answered";
      reply(request.id, { status: "resolved", approvalId: "approval-A", approved: request.params.approved });
      break;
    }
    case "audit/emit":
      // Controlled client lifecycle tests only; not a substitute for AppCore's
      // approval authorization or terminal policy. Default audit path is intact.
      for (const value of request.params.events) event(value);
      reply(request.id, {});
      break;
    case "audit/release-receipts":
      for (const pending of withheld.splice(0)) reply(pending.id, { status: "resolved",
        approvalId: pending.params.approvalId, approved: pending.params.approved });
      reply(request.id, {});
      break;
    case "audit/finish": {
      const turn = turns.get(request.params.threadId);
      if (!turn || turn.state !== "running") throw new Error("only a running nonwaiting turn can complete");
      turn.state = "completed";
      event({ type: "turn.completed", threadId: turn.threadId, turnId: turn.turnId,
        message: "B completed independently" });
      reply(request.id, {});
      break;
    }
    case "audit/barrier":
      reply(request.id, { turns: [...turns.values()], approvalResponses, responses, withheld: withheld.length });
      break;
    case "audit/exit":
      reply(request.id, {});
      clearTimeout(watchdog);
      process.stdin.pause();
      process.exit(0);
      break;
    default:
      send({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "unknown fixture method" } });
  }
});
