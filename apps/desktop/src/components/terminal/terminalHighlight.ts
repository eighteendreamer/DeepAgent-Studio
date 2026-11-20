import type { Terminal, IDecoration, IMarker } from "@xterm/xterm";
import type { HighlightRule } from "./terminalPreferences";
import type { HighlightHit, HighlightLine } from "./highlightRegex";

export class TerminalHighlighter {
  private worker: Worker | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private deadline: ReturnType<typeof setTimeout> | null = null;
  private decorations: (IDecoration | IMarker)[] = [];
  private failed = false;
  private dirty = false;
  constructor(private term: Terminal, private rules: HighlightRule[], private error: (message: string) => void) {}
  configure(rules: HighlightRule[]) { this.rules = rules; this.failed = false; this.stopWorker(); this.clear(); this.schedule(); }
  schedule() {
    if (this.failed || this.timer || !this.rules.some(rule => rule.enabled)) return;
    this.timer = setTimeout(() => { this.timer = null; this.render(); }, 200);
  }
  private render() {
    if (this.worker) { this.dirty = true; return; }
    this.dirty = false;
    const buffer = this.term.buffer.active;
    if (buffer.type !== "normal") { this.clear(); return; }
    const lines: HighlightLine[] = [];
    for (let y = buffer.viewportY; y < Math.min(buffer.viewportY + this.term.rows, buffer.length, buffer.viewportY + 160); y++) {
      lines.push({ line: y, text: buffer.getLine(y)?.translateToString(true).slice(0, 2048) ?? "" });
    }
    const worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" }); this.worker = worker;
    const fail = (message: string) => { this.failed = true; this.stopWorker(); this.clear(); this.error(message); };
    this.deadline = setTimeout(() => fail("Highlight processing exceeded its time limit"), 500);
    worker.onerror = () => fail("Highlight worker failed");
    worker.onmessage = (event: MessageEvent<{ hits?: HighlightHit[]; error?: string }>) => {
      this.stopWorker(); this.clear();
      if (event.data.error) { fail(event.data.error); return; }
      if (this.term.buffer.active !== buffer || buffer.viewportY !== lines[0]?.line) { this.schedule(); return; }
      for (const hit of event.data.hits ?? []) {
        const row = this.term.buffer.active.getLine(hit.line); if (!row) continue;
        if (row.translateToString(true).slice(0, 2048) !== lines.find(line => line.line === hit.line)?.text) continue;
        // RegExp indices are UTF-16 offsets; xterm decorations use display cells.
        let offset = 0, start = -1, end = -1;
        for (let x = 0; x < row.length; x++) {
          const cell = row.getCell(x); if (!cell || cell.getWidth() === 0) continue;
          const length = cell.getChars().length || 1;
          if (start < 0 && offset + length > hit.start) start = x;
          if (offset < hit.end) end = x + Math.max(1, cell.getWidth());
          offset += length;
        }
        if (start < 0 || end <= start) continue;
        const marker = this.term.registerMarker(hit.line - (this.term.buffer.active.baseY + this.term.buffer.active.cursorY));
        if (!marker) continue;
        this.decorations.push(marker);
        const decoration = this.term.registerDecoration({ marker, x: start, width: Math.min(end - start, this.term.cols - start), backgroundColor: hit.color, foregroundColor: "#111111", layer: "top" });
        if (decoration) this.decorations.push(decoration);
      }
      if (this.dirty) this.schedule();
    };
    worker.postMessage({ lines, rules: this.rules });
  }
  private clear() { for (const decoration of this.decorations) decoration.dispose(); this.decorations = []; }
  private stopWorker() { this.worker?.terminate(); this.worker = null; if (this.deadline) clearTimeout(this.deadline); this.deadline = null; }
  dispose() { if (this.timer) clearTimeout(this.timer); this.timer = null; this.stopWorker(); this.clear(); }
}
