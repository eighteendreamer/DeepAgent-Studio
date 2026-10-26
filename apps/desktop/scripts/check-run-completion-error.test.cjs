const test = require("node:test");
const assert = require("node:assert/strict");
const { loadTypeScript } = require("./audit-workflow-contract.cjs");

function completionTransport(completion) {
  const handlers = new Map();
  return {
    handlers,
    imports: {
      "@tauri-apps/api/core": {
        invoke: async (command) => {
          assert.equal(command, "start_chat_v2");
          handlers.get("session://completed")?.({ payload: completion });
          return { run_id: completion.run_id, session_id: completion.session_id ?? null, accepted: true };
        },
      },
      "@tauri-apps/api/event": {
        listen: async (event, callback) => {
          handlers.set(event, callback);
          return () => {
            if (handlers.get(event) === callback) handlers.delete(event);
          };
        },
      },
    },
  };
}

async function loadClient(completion) {
  const transport = completionTransport(completion);
  const previousWindow = globalThis.window;
  globalThis.window = { __TAURI_INTERNALS__: {} };
  const client = loadTypeScript("../src/api.ts", transport.imports);
  return { client, transport, restore: () => {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  } };
}

test("runChat preserves failed completion status for a late persistence error", async () => {
  const loaded = await loadClient({
    run_id: "audit-run-failed",
    session_id: "audit-session",
    status: "failed",
    error: "audit injected cancel audit storage failure",
  });
  try {
    await assert.rejects(
      loaded.client.runChat("audit", () => {}, undefined, null, "audit-run-failed"),
      (error) => {
        assert.equal(error.name, "RunCompletionError");
        assert.equal(error.status, "failed");
        assert.equal(error.runId, "audit-run-failed");
        assert.match(error.message, /audit injected cancel audit storage failure/);
        return true;
      },
    );
  } finally {
    loaded.restore();
  }
});

test("runChat keeps expected cancellation distinct from failed persistence", async () => {
  const loaded = await loadClient({
    run_id: "audit-run-cancelled",
    session_id: "audit-session",
    status: "cancelled",
    error: "cancelled by user",
  });
  try {
    await assert.rejects(
      loaded.client.runChat("audit", () => {}, undefined, null, "audit-run-cancelled"),
      (error) => {
        assert.equal(error.name, "RunCompletionError");
        assert.equal(error.status, "cancelled");
        assert.equal(error.message, "cancelled by user");
        return true;
      },
    );
  } finally {
    loaded.restore();
  }
});
