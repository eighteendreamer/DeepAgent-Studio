import {
  terminalSessionOpen, terminalSessionRead, terminalSessionWrite, terminalSessionResize,
  terminalSessionRenew, terminalSessionClose,
  type TerminalSession, type TerminalInputLease, type TerminalTarget,
} from "../../api";

export type TerminalControllerState = "starting" | "running" | "exited" | "error";

interface TerminalSink {
  /** Resolve only when xterm has parsed this batch, not when it was merely queued. */
  write(text: string): Promise<void>;
  state(state: TerminalControllerState, error?: string): void;
  gap(): void;
}

/** One mounted UI tab, one backend session, one byte cursor and one streaming decoder. */
export class TerminalSessionController {
  private current: [TerminalSession, TerminalInputLease] | null = null;
  private disposed = false;
  private stopped = false;
  private started = false;
  private cursor = 0;
  private decoder = new TextDecoder("utf-8");
  private readTimer: ReturnType<typeof setTimeout> | null = null;
  private renewTimer: ReturnType<typeof setTimeout> | null = null;
  private idleDelay = 30;
  private inputs: Promise<void> = Promise.resolve();
  private lastSize: { cols: number; rows: number } | null = null;

  constructor(
    private target: TerminalTarget,
    private runId: string,
    private sink: TerminalSink,
  ) {}

  async open(cols: number, rows: number): Promise<void> {
    if (this.started || this.disposed) throw new Error("Terminal controller cannot be reopened");
    this.started = true;
    this.sink.state("starting");
    try {
      const current = await terminalSessionOpen(this.runId, cols, rows, this.target);
      if (this.disposed) {
        await terminalSessionClose(...current);
        return;
      }
      this.current = current;
      this.lastSize = { cols, rows };
      this.sink.state("running");
      this.scheduleRead(0);
      this.scheduleRenewal();
    } catch (error) {
      if (!this.disposed) this.fail(error);
      throw error;
    }
  }

  private scheduleRead(delay: number) {
    if (this.disposed || this.stopped) return;
    this.readTimer = setTimeout(() => this.read(), delay);
  }

  private async read(): Promise<void> {
    this.readTimer = null;
    const current = this.current;
    if (!current || this.disposed || this.stopped) return;
    try {
      const chunk = await terminalSessionRead(current[0], this.cursor);
      if (this.disposed || this.stopped || this.current !== current) return;
      if (chunk.truncated) {
        this.decoder = new TextDecoder("utf-8");
        this.sink.gap();
      }
      const text = this.decoder.decode(new Uint8Array(chunk.data), { stream: !chunk.eof });
      if (text) await this.sink.write(text);
      if (this.disposed || this.stopped || this.current !== current) return;
      this.cursor = chunk.cursor;
      if (chunk.eof) {
        this.stop();
        this.sink.state("exited");
        return;
      }
      this.idleDelay = chunk.data.length ? 0 : Math.min(180, this.idleDelay + 30);
      this.scheduleRead(document.hidden ? 750 : this.idleDelay);
    } catch (error) {
      if (!this.disposed) this.fail(error);
    }
  }

  private scheduleRenewal() {
    if (this.disposed || this.stopped) return;
    // Existing production leases last 30 minutes. Renew explicitly, never take over silently.
    this.renewTimer = setTimeout(async () => {
      this.renewTimer = null;
      const current = this.current;
      if (!current || this.disposed || this.stopped) return;
      try {
        await terminalSessionRenew(...current);
        if (!this.disposed && this.current === current) this.scheduleRenewal();
      } catch (error) {
        if (!this.disposed) this.fail(error);
      }
    }, 60_000);
  }

  private enqueue(operation: (session: TerminalSession, lease: TerminalInputLease) => Promise<void>) {
    const current = this.current;
    const work = this.inputs.then(async () => {
      if (!current || this.disposed || this.stopped || this.current !== current) return;
      await operation(...current);
    });
    // Retain a settled ordering tail; the returned operation still rejects and displays its error.
    this.inputs = work.catch(() => {});
    return work.catch((error: unknown) => {
      if (!this.disposed) this.fail(error);
      throw error;
    });
  }

  write(data: string): Promise<void> {
    return this.enqueue((session, lease) => terminalSessionWrite(session, lease, data));
  }

  resize(cols: number, rows: number): Promise<void> {
    return this.enqueue(async (session, lease) => {
      if (this.lastSize?.cols === cols && this.lastSize.rows === rows) return;
      await terminalSessionResize(session, lease, cols, rows);
      this.lastSize = { cols, rows };
    });
  }

  private stop() {
    this.stopped = true;
    if (this.readTimer !== null) clearTimeout(this.readTimer);
    if (this.renewTimer !== null) clearTimeout(this.renewTimer);
    this.readTimer = this.renewTimer = null;
  }

  private fail(error: unknown) {
    this.stop();
    this.sink.state("error", error instanceof Error ? error.message : String(error));
  }

  async close(): Promise<void> {
    this.disposed = true;
    this.stop();
    const current = this.current;
    this.current = null;
    if (current) await terminalSessionClose(...current);
  }
}
