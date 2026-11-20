export type TerminalMetadataEvent =
  | { kind: "title"; title: string }
  | { kind: "bell" }
  | { kind: "directory"; path: string }
  | { kind: "integration"; status: "waiting" | "ready" | "unavailable"; shell?: string }
  | { kind: "commandStart"; command: string; at: number }
  | { kind: "commandEnd"; command: string; at: number; duration: number; exitCode: number | null; cwd: string };

export function safeTerminalLabel(value: string): string {
  return value.replace(/[\x00-\x1f\x7f-\x9f\u202a-\u202e\u2066-\u2069]/g, "").slice(0, 160);
}
function decode(value: string): string {
  if (value.length > 24 * 1024) throw new Error("metadata limit");
  return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(atob(value), c => c.charCodeAt(0)));
}
function validPath(path: string) {
  return path.length <= 16 * 1024 && !/[\x00-\x1f\x7f]/.test(path) && (path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path));
}

/** Metadata is advisory display data; it never authorizes host commands or input. */
export class TerminalMetadata {
  private nonce = "";
  private ready = false;
  private pending: { command: string; at: number } | null = null;
  constructor(private emit: (event: TerminalMetadataEvent) => void, private remote = false) {}
  expectIntegration(nonce: string) {
    if (!/^[A-Za-z0-9-]{8,128}$/.test(nonce)) throw new Error("invalid integration nonce");
    this.nonce = nonce; this.ready = false; this.pending = null;
    this.emit({ kind: "integration", status: "waiting" });
  }
  gap() {
    this.ready = false; this.nonce = ""; this.pending = null;
    this.emit({ kind: "integration", status: "unavailable" });
  }
  bell() { this.emit({ kind: "bell" }); }
  osc(code: number, value: string): boolean {
    if (value.length > 32 * 1024) return true;
    try {
      if (code === 0 || code === 2) { this.emit({ kind: "title", title: safeTerminalLabel(value) }); return true; }
      if (code === 7) {
        const url = new URL(value);
        if (url.protocol !== "file:" || url.username || url.password || url.search || url.hash) return true;
        let path = decodeURIComponent(url.pathname);
        if (!this.remote && /^\/[A-Za-z]:\//.test(path)) path = path.slice(1);
        if (validPath(path)) this.emit({ kind: "directory", path });
        return true;
      }
      if (code !== 777) return false;
      const [prefix, nonce, action, ...fields] = value.split(";");
      if (prefix !== "deepagent" || !this.nonce || nonce !== this.nonce) return true;
      if (action === "ready") {
        this.ready = true;
        this.emit({ kind: "integration", status: "ready", shell: safeTerminalLabel(fields[0] ?? "") });
      } else if (action === "unsupported") { this.gap(); }
      else if (this.ready && action === "begin" && fields.length === 1) {
        const command = decode(fields[0]);
        if (!command.trim() || command.length > 8192 || /[\x00\x1b]/.test(command)) return true;
        this.pending = { command, at: Date.now() };
        this.emit({ kind: "commandStart", ...this.pending });
      } else if (this.ready && (action === "end" || action === "prompt") && fields.length === 2) {
        const cwd = decode(fields[1]); if (!validPath(cwd)) return true;
        this.emit({ kind: "directory", path: cwd });
        if (action === "end" && this.pending) {
          const exitCode = /^\d{1,10}$/.test(fields[0]) ? Number(fields[0]) : null;
          this.emit({ kind: "commandEnd", ...this.pending, exitCode, cwd, duration: Math.max(0, Date.now() - this.pending.at) });
          this.pending = null;
        }
      }
    } catch { /* Malformed remote metadata cannot break rendering or execute anything. */ }
    return true;
  }
}
