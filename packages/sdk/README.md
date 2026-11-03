# @deepagent/sdk

TypeScript client for the DeepAgent Harness app-server.

The default transport starts `deepagent server --transport stdio`. A custom
binary can be supplied with `command`, `args`, and `cwd`.

```ts
import { DeepAgent } from "@deepagent/sdk";

const agent = new DeepAgent({ transport: "stdio" });
const thread = await agent.startThread({ cwd: process.cwd() });
for await (const event of thread.runStreamed("inspect the repository")) {
  console.log(event);
}
await agent.close();
```

`cli-jsonl` is available for one-shot automation through `runJsonl(prompt)`.
Thread lifecycle and turn control require the stdio app-server transport.

`Thread.runStreamed` owns a subscription **before** sending `turn/start` and
retains candidate events while the response identity is not yet known. It then
delivers only the returned turn, rejects conflicting explicit thread IDs, and
stops at its first completed/failed/interrupted event. Existing v1 events may
omit `threadId`; their `turnId` must still match. The per-operation queue is
bounded to 256 events and 4 MiB of serialized UTF-8 event data, including the
pre-response window. Overflow and transport loss reject the stream explicitly;
they do not mean the remote turn failed, completed, or never executed.

The returned iterator is lazy. `return()` releases parked local consumers,
including while admission is unanswered, and does not start an unused iterator.
An optional `{ signal: AbortSignal }` in `runStreamed` cancels local consumption;
the signal is not a protocol field. Neither cancellation nor `return()` silently
interrupts or resubmits a remote turn. If admission might have succeeded but its
reply was abandoned, reconcile `thread/read` before another start. Use
`Turn.interrupt()` when explicitly requesting remote cancellation of a known turn.
Reconnect ends old owned streams; it does not attach them to the new process or
automatically replay provisional live deltas. The lower-level `subscribe` is
only-from-now, not a late-subscription replay API.

Rust callers can use `thread_start_streamed` / `turn_start_streamed`, then consume
the returned `RequestEvents::recv()` **after** awaiting admission. These reuse
the existing 256-record broadcast channel; they fail explicitly on lag or EOF,
release their receiver on drop/close, and expose the original response and IDs.
Rust's record bound is not a byte/frame-size limit. Existing low-level request
methods and `subscribe()` remain available, but callers composing them must
subscribe first. These helpers do not add another event store, automatic ACK,
durable replay, or a timeout to the low-level pending-RPC table.

`pnpm -C packages/sdk test` runs the local stdio ordering/lifetime regressions
without a CLI build. For the explicit real-CLI stream/reopen/cursor gate, first
run `cargo build -p deepagent-cli --offline`, then
`pnpm -C packages/sdk test:stdio`. This gate requires the real binary (no skip),
verifies an identical executable copy in an empty installation, and uses fresh
synthetic data/home/workspace roots with no model configuration. It tests the
early-failure and durable-reconciliation path, not a configured provider or
packaged plugin/MCP startup. Rust's matching owned thread/failure tests also
isolate their test processes and executable copies; local cancellation never
implies rollback.

Within one app-server, `turn/start` starts only an idle thread. An admitted,
running, queued replacement or cancelling turn keeps the thread busy until its
worker finishes cleanup. A second ordinary start fails with JSON-RPC `-32005`
(TypeScript `HarnessRpcError.code`, Rust `SdkError::Rpc.code`): it does not create
a run, enqueue input, or cancel the existing turn. A terminal notification can
precede worker cleanup, so it is not by itself a new-start readiness receipt. Use
`turn/steer` only when you explicitly intend replacement. Different threads may
run concurrently. A lost response is not permission to resubmit: reconcile with
`thread/read` first. This busy check is process-local, not an idempotency key or
a cross-host exclusion guarantee; AppCore retains its existing lease/dispatch
protection across hosts.

New threads persist their explicit provider/model/permission/backend overrides
in the existing session file. Resume and fork retain these values; per-turn
overrides do not change thread defaults, but steer replacements retain that
turn's overrides (including reasoning effort). Unset values still use current
host/project settings, not a frozen copy of every runtime setting.

`thread.configuration` exposes the versioned explicit snapshot returned by
start/resume. `null` means an older/non-harness session has no saved snapshot:
its former in-memory overrides cannot be recovered, so review the host defaults
and explicitly override the next turn when needed. `undefined` means an older
server did not report configuration. Unknown persisted configuration versions
are rejected, not silently treated as defaults. This additive file metadata
does not require a SQLite migration; older binaries may ignore it, so do not
use a downgrade to preserve a restricted session's execution contract.

The stdio server has a bounded event queue. Overflow or output write/flush
failure terminates the connection with an error instead of silently continuing
after dropping notifications. It requests cancellation through the existing
runtime for that connection's active/queued turns and waits up to 10 seconds;
a cleanup timeout is an error, not proof that the turns reached a terminal.
Never automatically resubmit a turn after a transport failure: its side effects
may already have happened. Reconnect, then use `thread/read` to reconcile the
durable run status and history (for example via `requestForTest`). Persist the
returned session/run cursors, not the envelope's connection-local
`eventSequence`. `event/ack` is only a monotonic, in-range connection receipt;
it neither persists a cursor nor requests replay, and resets on reconnect.
The SDK does not automatically reconcile interrupted streams or pending approvals.

Live content/reasoning deltas and per-token stream metadata are provisional,
not durable commit receipts. Text may be batched in the existing run ledger;
replay is not a one-row-per-live-notification transcript. Canonical runtime
completion notifications from the Kernel, admission, and workflow paths are
delivered only after the durable terminal transaction commits. A provider
`response.completed` item is only a model-response boundary, not proof that the
run completed. A rejected persistence barrier stops the run before later
model/tool actions, but an earlier tool may already have executed. Transport or
admission errors are not durable receipts: after a disconnect, reconcile the
durable run terminal, action state, and session result with `thread/read` instead
of automatically repeating the action or treating a live completion as proof.
