# @deepagent/sdk

TypeScript client for the DeepAgent Harness app-server.

## Effective execution configuration

`startThread({reasoningEffort: "none"})` persists an explicit thread default
through resume, fork and host restart. Omission (or JSON null) keeps the existing
host/project default; the string `"none"` explicitly disables thinking.
`thread.run(input, {reasoningEffort: "high"})` overrides only that turn, including
its tool loop and steer replacement; it does not rewrite the thread default.
The existing model resolver validates the canonical `none`/`low`/`high`/`max`
values in the shared model resolver, not thread creation. A successful creation only
acknowledges saved input, not model/backend/credential readiness; invalid nonblank strings
are preserved and reported as invalid by scoped `readConfig`. The existing
turn lifecycle acknowledges the invalid plan and persists a failed terminal
before any provider request; it does not silently fall back to a default effort.
Empty/whitespace-only strings retain the
existing shared overlay's inherit-default behavior (not explicit `"none"`).
Compatibility aliases remain unsupported
(H-155); no new provider parser is introduced. Rust `ThreadStartRequest` literals
must supply `reasoning_effort: None` to retain the old omitted-field behavior.

`await agent.readConfig()` reads defaults for the **host workspace, including
its project layers**, not the last thread or machine-wide project-free settings.
Use `readConfig({threadId})` for persistent thread overrides (also before resume),
or `readConfig({cwd})` for another workspace's defaults; selectors are exclusive.
The versioned result reports the resolved model/reasoning, permission profile,
sandbox selection, source scopes, trust, MCP publication and plugin digest.
It is a limited projection, not raw settings, prompts, paths or credentials.
`resolutionStatus: "resolved"` does **not** mean execution is ready: credentials
are deliberately unchecked, backend admission can still fail, and actual tools
belong to the existing admitted `listTools` capability snapshot.

```ts
const config = await agent.readConfig({threadId: thread.id});
const turn = await thread.run("inspect the project", {
  expectedConfigSnapshotId: config.configSnapshotId
});
```

The optional guard compares the exact prepared model/policy/tool-search and host
publication inputs inside admission. Drift (including explicit different turn
overrides) fails before publishing a run; read again rather than retrying blindly.
Builtin slash controls have no model configuration snapshot: omit this guard.
Accepted normal chat consumes the frozen model defaults and policy, even if
settings change before the worker starts. Credentials are bound only at execution
and are never frozen or exposed. This is not an all-settings/MVCC transaction or
an authorization token. Config load warnings are counts, never raw diagnostics;
unknown model overrides retain the existing catalog fallback and report whether
the override applied. Cold reads do not resume, admit, refresh plugins or start MCP;
legacy settings normalization remains owned by the existing Settings service.
MCP publication identity changes across host restarts, so read a fresh snapshot
after reconnecting; unchanged model/policy values alone do not preserve its ID.
Uninitialized settings retain the existing `deepseek-official` provider banner
but have null model/reasoning; invalid model configuration explicitly reports
`resolutionStatus: "invalid"` and null provider/model rather than a usable plan.
Rust callers retain `config_read()` and gain `config_read_scoped(ConfigReadRequest)`;
request wire fields are additive, while Rust `TurnStartRequest` literals need the new
`expected_config_snapshot_id: None` field when no comparison guard is desired.
Use `ConfigReadRequest::default()` in place of old empty struct literals, or
supply the new optional `thread_id`/`cwd` fields explicitly.
The TS return type is a compile-time projection, not a new runtime schema validator.

Both SDKs validate `initialize` before allowing business requests: the exact
machine `protocolVersion` (currently 1), nonblank `serverName`/`serverVersion`,
and six boolean declarations in `capabilities` (`threadLifecycle`, `streaming`,
`interrupt`, `steer`, `approval`, `reconnect`) are required. Missing declarations
or incompatible results now fail early, even if the JSON-RPC response reports
success. A false declaration and unknown extension fields remain valid; these
flags are not permission grants or proof of provider/backend readiness, and
do not add per-method capability enforcement. Rust retains the raw `RpcResponse`
and reports `InvalidInitializeResult`/`IncompatibleProtocolVersion`; TypeScript
reports `HarnessProtocolError` with safe field names and numeric version details,
not arbitrary peer JSON. Rejected or late duplicate replies do not authorize
calls. Handshake validation does not retry or downgrade versions; the explicit
connection recovery policy is documented below. Existing incomplete mock peers must
publish the complete v1 handshake contract. Rust error enum exhaustive matches
may need updating. Business result validation and transport restart are separate.

Mutation receipts are also validated before a request resolves: `thread/start`,
`thread/resume`, `thread/fork`, `thread/archive`, `turn/start`, `turn/interrupt`,
`turn/steer`, `approval/respond` and `event/ack` require their typed v1 identity,
status and control fields. Echoed identities/decisions/ACKs must match the request;
steer identifies a new turn and the replaced turn. Both raw and owned APIs use
this gate; read-only history/config/tool/sandbox projections keep their existing
contracts. Unknown execution metadata and optional legacy configuration/snapshot
fields remain untouched in the original successful response. Incomplete synthetic
peers must publish the actual v1 receipt (including status); no wire field or
protocol-version migration is introduced.

Rust reports `SdkError::InvalidMethodResult(ControlResultError)` and TypeScript
reports `HarnessResultError` (separate from transport/RPC errors). They carry safe
method/field/expected-shape/received-type diagnostics, never arbitrary peer values.
Owned admission failures now report this same error before constructing a stream;
local stream-scope defense retains Rust's `InvalidStreamIdentity`. Rust exhaustive
error matches and TS consumers catching only transport errors may need updating.
An invalid success receipt does **not** prove the server performed no mutation:
reconcile durable state before retrying. The SDK does not redispatch, interrupt,
ACK, or reconstruct missing identities. Rust validates the full unsigned 64-bit
ACK range; TS rejects non-safe-integer receipt fields (`acknowledgedEventSequence`
and `forkedAt`) instead of claiming precision it cannot represent. Read-only
projections are not made fully schema-
validated by this change, and transport restart remains separate.

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

### CLI JSONL process ownership

`runJsonl(prompt, { signal })` is lazy: an unused iterator starts no process.
Early `return()` / `for await` break, consumer `throw()`, malformed JSONL,
local abort and `await agent.close()` share one owner cleanup boundary. They
close local readers, terminate the directly spawned CLI if needed and await
its actual exit/stream-close receipt. `return()` and close can preempt a parked
`next()`; ending one invocation leaves concurrent invocations alone. Close
prevents future launches, waits for every active owner and is idempotent.
Unconfirmed termination rejects, retains the owner, and permits explicit close
retry; sending a signal or `child.killed` alone is not an exit receipt.

Normal consumption drains stderr and waits for real process completion, not
just `turn.completed` or stdout EOF (the CLI may still be saving session memory).
After process exit, pipes must close within one second; inherited pipes still
held by an unmanaged descendant produce an explicit error instead of hanging.
On Windows local termination is forced, not a cooperative runtime interrupt.
On POSIX cleanup allows up to one second for SIGTERM before SIGKILL, then up
to one second for exit and one for stream closure. These are timer bounds,
not real-time scheduling guarantees. SIGTERM escalation has not been exercised
on this Windows host.

This contract owns the **direct process**, not arbitrary custom wrapper trees,
detached jobs, remote effects, or a new durable cancellation mechanism. Native
tool/MCP process owners retain their existing responsibilities. Reaping the CLI
does not prove its run was cancelled, its session-memory pass completed, or
earlier side effects rolled back. Reconcile persistent state before retrying;
use stdio turn/interrupt when business cancellation is needed.

On full consumption, a nonzero exit or an external termination signal rejects
with `HarnessTransportError`, **even after a business terminal was delivered**.
Already yielded events remain unchanged; the SDK never appends a synthetic
success/failure/cancellation event or automatically retries the invocation.
The error message records the native code/signal. Its `cause` contains only
`{ code: number | null, signal: string | null, stderrPresent: boolean }`.
Raw stderr is not retained in the error or a second buffer; opt into `onStderr`
to handle it under your own redaction/logging policy. Spawn, parse, pipe and
callback failures retain their original causes instead of being replaced by
the exit status of cleanup's kill.

Intentional local return/close preserves its existing completion contract and
is not proof of process or business success. A close after an already observed
natural failure cannot hide that failure from the invocation. A zero exit is
only a process-level receipt: `runJsonl` is a raw event projection, so callers
still need to inspect business terminal/error events and durable state. Empty
or partial zero-exit output is not upgraded to a successful turn.

`pnpm -C packages/sdk test:jsonl` is an explicit real-CLI gate (no skipped
fallback), after `cargo build -p deepagent-cli --offline`. Its SQLite test
helper needs Node >=22.13; production SDK still supports Node >=20 without
that module. It runs in a fresh synthetic home/data/workspace with an identical
executable copy and no provider settings: normal `/help`, uninitialized-model
failure plus native argument/startup nonzero failures before events, forced
close during a held synthetic SQLite writer gate, and durable
reopen without redispatch or invented cancellation. It does not test a real
provider, an interrupted in-flight model/tool, packaged plugins or arbitrary
descendant-tree cleanup. Default tests separately exercise real Node children,
concurrent owners, termination refusal/retry and detached inherited-pipe limits.

## Bounded durable history

`readThreadPage(threadId, options)` fetches **one** `thread/read` page; it does
not accumulate history or retry. Rust callers use `thread_read(ThreadReadRequest)`
and inspect the same `hasMore` / `nextCursor` fields. Consume or persist each page
before requesting the next, and stop at `hasMore: false`:

```ts
let cursor: string | undefined; // Or restore your saved checkpoint.
for (let pageNumber = 0; pageNumber < 10; pageNumber++) {
  const page = await agent.readThreadPage(thread.id, {
    cursor, pageSize: 100, maxBytes: 262144
  });
  // Process only this page. A run's events/controls/graph can span several pages;
  // merge slices by runId and their row identities instead of replacing the run.
  console.log(page.events, page.runEvents);
  cursor = page.nextCursor ?? undefined;
  if (!page.hasMore) break;
}
// Save cursor if present: this window stopped before the complete history.
```

`pageSize` is 1..500 (default 500), counting session events, run envelopes and
each event/action/approval/artifact/graph node together. `maxBytes` is 4096..1 MiB
(default 256 KiB), counting the complete successful JSON-RPC response, not just
event bodies. A complete source/native group and its envelope must fit; otherwise
the server returns an explicit error, never a partial group or silent truncation.
A continued run still needs its envelope, so very small budgets can be insufficient.

The opaque cursor is scoped to the database and canonical thread, survives process
restart and fixes a session-file generation/byte cut and SQLite insertion high-waters.
It is not a credential, approval or execution grant. Keep it opaque; tampered,
foreign, overlong or rewound-session continuations are rejected. Repeating a cursor
is an allowed read-only retry, not automatic redispatch; token inequality is not
proof of row progress. The SDK intentionally provides no unbounded pagination loop.

Initial `afterSequence`, `sessionAfterSequence`, `runAfterSequence` and per-run
filters keep their incremental meaning. On continuation omit them or repeat the
same filters; changing them is rejected. An explicit sequence filter can start
inside a historical group; only server-created page cuts are group aligned.
Control states/leases/metadata are live projections, not an MVCC historical snapshot.
The session-file cut and SQLite cut are not one cross-store atomic transaction.
SQLite high-water identity anchors reject deletion/reuse of a cut row with a stale
cursor error; restart hydration explicitly. Middle deletions may omit deleted
rows, while replacements inserted beyond the cut are excluded. Relationship
predicates remain live: a pre-existing resumed subagent can appear under its new
parent and its original parent. This is not frozen first-page graph membership.
Maintenance that changes the anchor is rejected, not silently restarted.

Legacy raw Event records do not encode source/native ownership. Plain legacy
messages can be paged individually. Adjacent exact source/native mirrors are kept
together using the existing projection mapping; native-first blocks use the
existing native-source matcher and remain atomic through their source message.
An indivisible block, unresolved lookahead, or individual JSONL record exceeding
8 MiB is rejected read-only. Old rewritten generations containing many small
records in a large frame are decoded incrementally: only a window of at most
501 records / 8 MiB of encoded record data is retained, with an 8 MiB decoder
window. The complete frame must pass its checksum before any selected window is
returned, including corruption after that window. No migration or rewrite occurs.
Continuations rescan a large old frame in O(frame size) work. The 256-frame /
32 MiB scan thresholds are checked between windows, not a hard CPU, I/O or elapsed
time limit; checksum validation may scan a larger complete frame. These allocation
boundaries are not a measured total-process RSS guarantee. Increasing maxBytes
cannot raise the individual-record limit.

The wire fields are additive, but a previous single-call reader must now follow
continuations or explicitly handle the bounded page/error. Existing small histories
keep their arrays. Rust struct literals need the three new optional fields.
The TypeScript page API rejects older, unpaged server responses instead of treating
them as a complete page. Final integration acceptance is tracked under H-218 in
`docs/system-harness-repair-progress.md`.

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
### Explicit stdio recovery

Each stdio transport owns one physical process generation and one shared,
validated initialization promise. Concurrent first requests do not send duplicate
`initialize` calls. Spawn, stdin/stdout/stderr stream errors, malformed stdout,
stdout closure and child exit make that transport terminal. Subsequent requests
reject with the recorded transport failure; they do **not** silently spawn a new
process with the previous generation's handshake. This intentionally replaces
the old implicit restart behavior. Spawn/pipe causes remain attached to
`HarnessTransportError`; ordinary server RPC errors are still `HarnessRpcError`.
A failed handshake stays unready and is not automatically retried on that process.

Call `await agent.reconnect()` to explicitly create a fresh, lazy transport. Its
first request performs a new validated handshake; concurrent reconnect calls
coalesce. Resume the desired persisted thread before starting new work, and read
durable history to reconcile any earlier request whose outcome is unknown. No
turn, approval response, ACK or interrupted stream is replayed for you. Await
reconnect before constructing an owned stream. Old replies/failure callbacks and
approval handlers cannot authorize or send a response on the replacement
transport. The existing global pending-approval completion barrier is separate
(H-295); reconnect does not cancel arbitrary user approval callbacks.

`close()` leaves the current transport closed and supersedes a pending reconnect;
it does not silently reopen on a later business request. A subsequent *explicit*
reconnect can deliberately reopen it. The stdio close path still requests direct
child termination; it is not a new process-tree teardown or an exit receipt.
Public request/result/wire types are unchanged. CLI JSONL lifecycle and exit-code
semantics are not changed by this stdio policy.

Never automatically resubmit a turn after a transport failure: its side effects
may already have happened. Reconnect, then use `thread/read` to reconcile the
durable run status and history via `readThreadPage`. Persist the
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
