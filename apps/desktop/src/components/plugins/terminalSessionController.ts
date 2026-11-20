import {
  terminalSessionOpen, terminalSessionRead, terminalSessionWrite, terminalSessionResize,
  terminalSessionRenew, terminalSessionClose, terminalSessionSubscribe,
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
  private closing: Promise<void> | null = null;
  private disposed = false;
  private stopped = false;
  private started = false;
  private cursor = 0;
  private decoder = new TextDecoder("utf-8");
  private readTimer: ReturnType<typeof setTimeout> | null = null;
  private renewTimer: ReturnType<typeof setTimeout> | null = null;
  private unsubscribe: (() => void) | null = null;
  private reading = false;
  private readRequested = false;
  private inputs: Promise<void> = Promise.resolve();
  private lastSize: { cols: number; rows: number } | null = null;

  constructor(
    private target: TerminalTarget,
    private runId: string,
    private sink: TerminalSink,
    private cwd?: string,
  ) {}

  async open(cols: number, rows: number): Promise<void> {
    if (this.started || this.disposed) throw new Error("Terminal controller cannot be reopened");
    this.started = true;
    this.sink.state("starting");
    try {
      const current = await terminalSessionOpen(this.runId, cols, rows, this.target, this.cwd);
      this.current = current;
      if (this.disposed) {
        await this.close();
        return;
      }
      this.lastSize = { cols, rows };
      const unsubscribe = await terminalSessionSubscribe(current[0], () => this.requestRead(), error => {
        if (!this.disposed) this.fail(error);
      });
      if (this.disposed || this.stopped) {
        unsubscribe();
        return;
      }
      this.unsubscribe = unsubscribe;
      this.sink.state("running");
      // Subscribe before draining: output arriving in either window must be read.
      this.requestRead();
      this.scheduleRenewal();
    } catch (error) {
      if (!this.disposed) this.fail(error);
      throw error;
    }
  }

  private requestRead() {
    if (this.disposed || this.stopped) return;
    if (this.reading) { this.readRequested = true; return; }
    if (this.readTimer === null) this.readTimer = setTimeout(() => this.read(), 0);
  }

  private async read(): Promise<void> {
    this.readTimer = null;
    const current = this.current;
    if (!current || this.disposed || this.stopped || this.reading) return;
    this.reading = true;
    this.readRequested = false;
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
      // A capped response can leave bytes ready; keep draining after parser ACK.
      this.readRequested ||= chunk.data.length > 0;
    } catch (error) {
      if (!this.disposed) this.fail(error);
    } finally {
      this.reading = false;
      if (this.readRequested) this.requestRead();
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
    this.unsubscribe?.();
    this.unsubscribe = null;
  }

  private fail(error: unknown) {
    this.stop();
    this.sink.state("error", error instanceof Error ? error.message : String(error));
  }

  close(): Promise<void> {
    this.disposed = true;
    this.stop();
    if (this.closing) return this.closing;
    const current = this.current;
    if (!current) return Promise.resolve();
    this.closing = terminalSessionClose(...current)
      .then(() => { this.current = null; })
      .finally(() => { this.closing = null; });
    return this.closing;
  }
}
