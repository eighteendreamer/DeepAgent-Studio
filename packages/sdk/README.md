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
