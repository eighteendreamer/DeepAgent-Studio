import { spawn, type ChildProcessByStdio } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import type { Readable } from "node:stream";
import type { CliRunOptions, DeepAgentOptions, HarnessEvent } from "./index.js";

type ErrorFactory = (message: string, cause?: unknown) => Error;
type CliChild = ChildProcessByStdio<null, Readable, Readable>;
const TERMINATE_GRACE_MS = 1000;
const REAP_GRACE_MS = 1000;

/** A timeout releases its timer/listener; a sent signal is never an exit receipt. */
async function within(task: Promise<void>, ms: number): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      task.then(() => true),
      new Promise<false>((resolve) => { timer = setTimeout(() => resolve(false), ms); })
    ]);
  } finally { clearTimeout(timer); }
}

/** Own only the spawned CLI and its readers, not a second runtime/cancel store. */
class CliProcessOwner {
  readonly lines: Interface;
  private readonly stderr: Interface;
  private readonly exited: Promise<void>;
  private readonly closed: Promise<void>;
  private readonly drained: Promise<void>;
  private hasExited = false;
  private disposal?: Promise<void>;
  private failure?: Error;
  private cleanupFailure?: unknown;
  private disposed = false;
  private readonly abort = () => this.fail(this.error("CLI JSONL execution aborted locally; reconcile durable state before retrying"));

  constructor(
    private readonly child: CliChild,
    private readonly error: ErrorFactory,
    private readonly signal?: AbortSignal,
    onStderr?: (line: string) => void
  ) {
    this.exited = new Promise((resolve) => {
      child.once("exit", () => { this.hasExited = true; resolve(); });
      child.on("error", (cause) => {
        this.failure ??= error("CLI JSONL process error", cause);
        // A failed spawn has no native process and emits no exit event.
        if (child.pid === undefined) { this.hasExited = true; resolve(); }
        this.lines?.close();
      });
    });
    this.closed = new Promise((resolve) => child.once("close", () => resolve()));
    this.lines = createInterface({ input: child.stdout });
    this.stderr = createInterface({ input: child.stderr });
    for (const stream of [child.stdout, child.stderr]) {
      stream.on("error", (cause) => this.fail(error("CLI JSONL pipe failed", cause)));
    }
    this.stderr.on("line", (line) => {
      try { onStderr?.(line); }
      catch (cause) { this.fail(error("CLI stderr handler failed", cause)); }
    });
    // Start from the process exit, not stdout EOF: an unmanaged descendant can
    // retain inherited pipes forever, so the reader loop may never reach EOF.
    this.drained = this.exited.then(async () => {
      if (!await within(this.closed, REAP_GRACE_MS)) {
        this.fail(error("CLI exited but its output streams remain open; descendant ownership is unresolved"));
      }
    });
    signal?.addEventListener("abort", this.abort, { once: true });
    if (signal?.aborted) this.abort();
  }

  async finish(): Promise<void> {
    // EOF is not necessarily process exit (e.g. session-memory is still draining).
    await this.drained;
    if (this.failure) throw this.failure;
    // Preserve trailing stderr delivery on normal completion. A descendant
    // retaining inherited pipes must not turn this root-exit edge into a hang.
    // H-266 exit-code/result validation is independent of ownership.
  }

  private fail(error: Error): void {
    this.failure ??= error;
    void this.stop().catch((failure) => { this.cleanupFailure = failure; });
  }

  stop(): Promise<void> {
    this.disposal ??= this.dispose().catch((failure) => {
      this.cleanupFailure = failure;
      this.disposal = undefined; // Explicit close can retry an unconfirmed reap.
      throw failure;
    });
    return this.disposal;
  }

  get released(): boolean { return this.disposed; }

  private async dispose(): Promise<void> {
    this.signal?.removeEventListener("abort", this.abort);
    this.lines.close(); // Wake a pending next, even if the child is silent.
    this.stderr.close();
    this.child.stdout.destroy();
    this.child.stderr.destroy();
    if (!this.hasExited) {
      // Windows maps both signals to forced termination, not cooperative cancel.
      if (process.platform !== "win32") {
        this.child.kill("SIGTERM");
        await within(this.exited, TERMINATE_GRACE_MS);
      }
      if (!this.hasExited) {
        const accepted = this.child.kill("SIGKILL");
        if (!await within(this.exited, REAP_GRACE_MS)) {
          throw this.error(`CLI JSONL process termination was not confirmed (signal ${accepted ? "accepted" : "refused"})`, this.cleanupFailure);
        }
      }
    }
    if (!await within(this.closed, REAP_GRACE_MS)) {
      throw this.error("CLI JSONL process streams did not close after exit");
    }
    this.disposed = true;
  }
}

/** The original one-shot transport, with one owner for every active invocation. */
export class CliJsonlTransport {
  private readonly active = new Set<CliProcessOwner>();
  private closed = false;
  private closing?: Promise<void>;

  constructor(private readonly options: DeepAgentOptions, private readonly error: ErrorFactory) {}

  run(prompt: string, options: CliRunOptions): AsyncIterableIterator<HarnessEvent> {
    let owner: CliProcessOwner | undefined;
    const transport = this;
    const generator = (async function* (): AsyncGenerator<HarnessEvent> {
      if (transport.closed) throw transport.error("CLI JSONL transport is closed");
      if (options.signal?.aborted) throw transport.error("CLI JSONL execution aborted before start");
      const child = spawn(transport.options.command ?? "deepagent",
        [...(transport.options.args ?? []), "run", "--json", prompt], {
          cwd: transport.options.cwd,
          stdio: ["ignore", "pipe", "pipe"],
          windowsHide: true
        });
      owner = new CliProcessOwner(child, transport.error, options.signal, transport.options.onStderr);
      transport.active.add(owner);
      let failure: unknown;
      try {
        for await (const line of owner.lines) {
          if (!line.trim()) continue;
          let event: HarnessEvent;
          try { event = JSON.parse(line) as HarnessEvent; }
          catch (cause) { throw transport.error("CLI emitted invalid JSONL", cause); }
          yield event; // A consumer throw is not a JSON parsing error.
        }
        await owner.finish();
      } catch (error) {
        failure = error;
        throw error;
      } finally {
        try { await owner.stop(); }
        catch (cleanup) {
          throw transport.error("CLI JSONL cleanup incomplete", failure === undefined
            ? cleanup : new AggregateError([failure, cleanup], "execution and cleanup failed"));
        } finally { if (owner.released) transport.active.delete(owner); }
      }
    })();
    // AsyncGenerator.return/throw normally queue behind next. Stop the owner
    // out-of-band first, so a parked reader cannot keep that queue stuck.
    const stop = async () => {
      if (owner) {
        await owner.stop();
        if (owner.released) transport.active.delete(owner);
      }
    };
    return {
      [Symbol.asyncIterator]() { return this; },
      next: () => generator.next(),
      return: async () => { await stop(); return generator.return(undefined); },
      throw: async (error) => { await stop(); return generator.throw(error); }
    };
  }

  close(): Promise<void> {
    this.closed = true; // No new child may appear after this boundary.
    this.closing ??= (async () => {
      const results = await Promise.allSettled([...this.active].map(async (owner) => {
        await owner.stop();
        if (owner.released) this.active.delete(owner);
      }));
      const failures = results.filter((result): result is PromiseRejectedResult => result.status === "rejected");
      if (failures.length) {
        throw this.error("CLI JSONL close could not reap every owned process", new AggregateError(failures.map((result) => result.reason)));
      }
    })().catch((failure) => { this.closing = undefined; throw failure; });
    return this.closing;
  }
}
