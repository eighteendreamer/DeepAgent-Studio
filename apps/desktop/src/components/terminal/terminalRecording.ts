import { recordingBudget, type TerminalBudget } from "./terminalBudget";
export type CastEvent = [number, "o" | "r", string];
export type TerminalCast = { width: number; height: number; events: CastEvent[]; duration: number };
const MAX_BYTES = 16 * 1024 * 1024, MAX_EVENTS = 100_000;
function dimensions(cols: number, rows: number) { return Number.isInteger(cols) && Number.isInteger(rows) && cols >= 2 && cols <= 1000 && rows >= 1 && rows <= 500; }
export class TerminalRecording {
  private events: CastEvent[] = []; private bytes = 0; private start: number;
  private disposed = false;
  constructor(private cols: number, private rows: number, private clock = () => performance.now(), private budget: TerminalBudget = recordingBudget) {
    if (!dimensions(cols, rows)) throw new Error("invalid recording dimensions"); this.start = clock();
  }
  private append(type: "o" | "r", data: string) {
    if (this.disposed) throw new Error("Recording was discarded");
    const size = new TextEncoder().encode(JSON.stringify(data)).length + 64;
    if (this.bytes + size > MAX_BYTES || this.events.length >= MAX_EVENTS) throw new Error("Recording reached the 16 MiB / 100000 event limit");
    if (!this.budget.reserve(size)) throw new Error("Terminal recordings reached the shared 64 MiB limit; export and discard a recording");
    this.bytes += size; this.events.push([Math.max(0, (this.clock() - this.start) / 1000), type, data]);
  }
  output(data: string) { if (data) this.append("o", data); }
  resize(cols: number, rows: number) { if (dimensions(cols, rows)) this.append("r", `${cols}x${rows}`); }
  dispose() { if (!this.disposed) { this.disposed = true; this.budget.release(this.bytes); this.bytes = 0; this.events = []; } }
  export(): string { return [JSON.stringify({ version: 2, width: this.cols, height: this.rows, timestamp: Math.floor(Date.now() / 1000), env: { TERM: "xterm-256color" } }), ...this.events.map(event => JSON.stringify(event))].join("\n") + "\n"; }
}
export function parseCast(text: string): TerminalCast {
  if (new TextEncoder().encode(text).length > MAX_BYTES + 1024) throw new Error("Recording size limit exceeded");
  const lines = text.trim().split(/\r?\n/); if (lines.length > MAX_EVENTS + 1) throw new Error("Recording event limit exceeded");
  const header = JSON.parse(lines.shift()!);
  if (header.version !== 2 || !dimensions(header.width, header.height)) throw new Error("Unsupported asciicast header");
  let previous = 0;
  const events: CastEvent[] = lines.map(line => {
    const e = JSON.parse(line);
    if (!Array.isArray(e) || e.length !== 3 || !Number.isFinite(e[0]) || e[0] < previous || e[0] > 7 * 86400) throw new Error("Invalid recording timestamp");
    if (!["o", "r"].includes(e[1]) || typeof e[2] !== "string") throw new Error("Only output and resize events may be replayed");
    if (e[1] === "r") { const m = /^(\d+)x(\d+)$/.exec(e[2]); if (!m || !dimensions(Number(m[1]), Number(m[2]))) throw new Error("Invalid resize event"); }
    previous = e[0]; return e as CastEvent;
  });
  return { width: header.width, height: header.height, events, duration: previous };
}
export type ReplaySink = { reset(): void; resize(cols: number, rows: number): void; write(text: string): void };
export function applyCastEvent(event: CastEvent, sink: ReplaySink) { if (event[1] === "o") sink.write(event[2]); else { const [cols, rows] = event[2].split("x").map(Number); sink.resize(cols, rows); } }
/** Has no controller, lease, input callback or live-session reference. */
export function replayAt(cast: TerminalCast, seconds: number, sink: ReplaySink) {
  sink.reset(); sink.resize(cast.width, cast.height);
  for (const e of cast.events) { if (e[0] > seconds) break; applyCastEvent(e, sink); }
}
export function downloadArtifact(name: string, data: BlobPart, type = "application/octet-stream") {
  const url = URL.createObjectURL(new Blob([data], { type }));
  const anchor = document.createElement("a"); anchor.href = url; anchor.download = name; anchor.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
