import Zmodem, { type Detection, type Session, type Offer } from "zmodem.js";
import { receiveBudget } from "./terminalBudget";

export type ZmodemNotice = { phase: "detected" | "offer" | "transferring" | "waiting"; role: "receive" | "send"; name?: string; size?: number; bytes?: number } | null;
export interface BinaryTerminalWriter { writeBytes(bytes: Uint8Array): Promise<void> }
export const MAX_ZMODEM_BYTES = 32 * 1024 * 1024;

export function safeZmodemName(name: string): string {
  if (!name || name === "." || name === ".." || name.length > 240 || /[\\/:\x00-\x1f\x7f]/.test(name) || /[. ]$/.test(name) || /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(name)) throw new Error("Unsafe ZMODEM filename");
  return name;
}

/** Protocol bytes stay ahead of UTF-8 decoding, recording and xterm parsing. */
export class TerminalZmodem {
  private sentry: InstanceType<typeof Zmodem.Sentry>;
  private detection: Detection | null = null;
  private session: Session | null = null;
  private offer: Offer | null = null;
  private output: Uint8Array[] = [];
  private writes = new Set<Promise<void>>();
  private disposed = false;
  private cancellation = new AbortController();
  private transferred = 0;
  private files = 0;
  private sending = false;
  private lastProgress = 0;
  private decoder = new TextDecoder("utf-8");

  constructor(
    private writer: BinaryTerminalWriter,
    private terminal: (text: string) => Promise<void>,
    private notice: (value: ZmodemNotice) => void,
    private received: (name: string, bytes: Uint8Array[]) => void,
    private error: (error: unknown) => void,
  ) { this.sentry = this.createSentry(); }

  private createSentry() {
    const signal = this.cancellation.signal;
    return new Zmodem.Sentry({
      to_terminal: bytes => { if (!this.disposed && !signal.aborted) this.output.push(Uint8Array.from(bytes)); },
      sender: bytes => {
        if (this.disposed || signal.aborted) return;
        const write = this.writer.writeBytes(Uint8Array.from(bytes));
        this.writes.add(write);
        void write.then(() => this.writes.delete(write), error => {
          this.writes.delete(write);
          if (!this.disposed && !signal.aborted) this.fail(error);
        });
      },
      on_detect: detection => {
        if (this.disposed || signal.aborted) return;
        this.detection = detection;
        this.notice({ phase: "detected", role: detection.get_session_role() });
      },
      on_retract: () => { this.detection = null; if (!this.session) this.notice(null); },
    });
  }

  get busy() { return !!(this.detection || this.session); }

  async consume(bytes: Uint8Array, eof = false): Promise<void> {
    if (this.disposed) return;
    try { this.sentry.consume(bytes); } catch (error) { this.fail(error); }
    const output = this.output; this.output = [];
    for (const part of output) {
      const text = this.decoder.decode(part, { stream: true });
      if (text) await this.terminal(text);
    }
    if (eof) {
      const tail = this.decoder.decode(); if (tail) await this.terminal(tail);
      this.dispose();
    }
  }

  private fail(error: unknown) { this.cancel(); this.error(error); }
  private async flush() { await Promise.all([...this.writes]); }
  private async cancellable<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
    if (signal.aborted) throw new Error("ZMODEM transfer cancelled");
    let stop!: () => void;
    const cancelled = new Promise<never>((_, reject) => {
      stop = () => reject(new Error("ZMODEM transfer cancelled"));
      signal.addEventListener("abort", stop, { once: true });
    });
    try { return await Promise.race([operation, cancelled]); }
    finally { signal.removeEventListener("abort", stop); }
  }

  confirm(): void {
    if (!this.detection || this.disposed) return;
    try {
      const session = this.detection.confirm(), signal = this.cancellation.signal;
      this.detection = null; this.session = session;
      this.transferred = 0; this.files = 0;
      session.on("session_end", () => {
        if (signal.aborted || this.session !== session) return;
        this.session = null; this.offer = null; this.notice(null);
      });
      this.notice({ phase: "waiting", role: session.type });
      if (session.type === "receive") {
        session.on("offer", offer => {
          if (signal.aborted) return;
          try {
            const { name, size } = offer.get_details(); safeZmodemName(name);
            if (++this.files > 16 || !Number.isSafeInteger(size) || size < 0 || this.transferred + size > MAX_ZMODEM_BYTES) throw new Error("ZMODEM session exceeds the 16 file / 32 MiB limit");
            this.offer = offer; this.notice({ phase: "offer", role: "receive", name, size });
          } catch (error) { offer.skip(); this.error(error); }
        });
        session.start();
      }
    } catch (error) { this.fail(error); }
  }

  skip() {
    try { this.offer?.skip(); this.offer = null; this.notice(this.session ? { phase: "waiting", role: this.session.type } : null); }
    catch (error) { this.fail(error); }
  }

  async acceptReceive() {
    const offer = this.offer, signal = this.cancellation.signal; if (!offer) return;
    const { name, size } = offer.get_details(); const chunks: Uint8Array[] = []; let bytes = 0, reserved = 0;
    this.offer = null; this.notice({ phase: "transferring", role: "receive", name, size, bytes: 0 });
    try {
      await this.cancellable(offer.accept({ on_input: payload => {
        if (signal.aborted) return;
        bytes += payload.length; this.transferred += payload.length;
        if (bytes > size || this.transferred > MAX_ZMODEM_BYTES) throw new Error("ZMODEM payload exceeds its declared size");
        if (!receiveBudget.reserve(payload.length)) throw new Error("Terminal downloads reached the shared 32 MiB limit");
        reserved += payload.length;
        chunks.push(Uint8Array.from(payload));
        if (performance.now() - this.lastProgress > 100) {
          this.lastProgress = performance.now();
          this.notice({ phase: "transferring", role: "receive", name, size, bytes });
        }
      } }), signal);
      if (signal.aborted || this.disposed) return;
      if (bytes !== size) throw new Error("ZMODEM file size mismatch");
      this.received(safeZmodemName(name), chunks);
      if (!this.offer && this.session) this.notice({ phase: "waiting", role: this.session.type });
    } catch (error) { if (!signal.aborted && !this.disposed) this.fail(error); }
    finally { receiveBudget.release(reserved); }
  }

  async sendFiles(files: File[]) {
    const session = this.session, signal = this.cancellation.signal;
    if (!session || session.type !== "send" || this.disposed || this.sending) throw new Error("The remote terminal is not ready to receive files");
    if (!files.length || files.length > 16 || files.reduce((sum, file) => sum + file.size, 0) > MAX_ZMODEM_BYTES) throw new Error("ZMODEM upload exceeds the 16 file / 32 MiB limit");
    for (const file of files) safeZmodemName(file.name);
    this.sending = true;
    try {
      for (const file of files) {
        const transfer = await this.cancellable(session.send_offer({ name: file.name, size: file.size }), signal);
        if (!transfer) continue;
        for (let offset = 0; offset < file.size; offset += 16 * 1024) {
          const bytes = new Uint8Array(await this.cancellable(file.slice(offset, offset + 16 * 1024).arrayBuffer(), signal));
          transfer.send(bytes); await this.flush();
          this.notice({ phase: "transferring", role: "send", name: file.name, size: file.size, bytes: Math.min(file.size, offset + bytes.length) });
        }
        await this.cancellable(transfer.end([]), signal); await this.flush();
      }
      await this.cancellable(session.close(), signal); await this.flush();
    } catch (error) {
      if (!signal.aborted && !this.disposed) { this.cancel(); throw error; }
    } finally { this.sending = false; }
  }

  /** Reset only the protocol; cancellation must leave the shell output usable. */
  cancel() {
    if (this.disposed) return;
    try { this.session?.abort(); this.detection?.deny(); } catch { /* Already ended. */ }
    this.cancellation.abort(); this.cancellation = new AbortController();
    this.session = null; this.detection = null; this.offer = null; this.sending = false;
    this.sentry = this.createSentry(); this.notice(null);
  }

  dispose() { if (!this.disposed) { this.cancel(); this.disposed = true; this.output = []; } }
}
