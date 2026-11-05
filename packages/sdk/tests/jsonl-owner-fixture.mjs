// Only synthetic files in the parent test's private temporary directory.
import { existsSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const [root, mode, run, json, id] = process.argv.slice(2);
if (run !== "run" || json !== "--json" || !/^[a-z-]+$/u.test(id)) {
  throw new Error("unexpected SDK-owned CLI arguments");
}
const file = (suffix) => path.join(root, `${id}.${suffix}`);
const worker = mode === "tree-worker";
writeFileSync(file(worker ? "worker-ready.json" : "ready.json"), JSON.stringify({ pid: process.pid }));
const emit = (type) => process.stdout.write(`${JSON.stringify({
  type, threadId: "synthetic-thread", turnId: id,
  item: { kind: "content_delta", text: "synthetic owner control" }, message: "done"
})}\n`);

if (mode === "ignore-term") process.on("SIGTERM", () => writeFileSync(file("term"), "received"));
if (mode === "tree") {
  spawn(process.execPath, [fileURLToPath(import.meta.url), root, "tree-worker", "run", "--json", id], {
    // Deliberate unmanaged custom wrapper: Windows libuv otherwise supplies its
    // own kill-on-parent-close job, which would not exercise the SDK boundary.
    stdio: ["ignore", process.stdout, process.stderr], windowsHide: true, detached: true
  });
}
if (mode !== "silent" && !worker) emit(mode === "terminal-wait" ? "turn.completed" : "item.updated");
const watchdog = setTimeout(() => process.exit(24), 8000);
const gate = setInterval(() => {
  if (existsSync(file("probe"))) writeFileSync(file(worker ? "worker-ack.json" : "ack.json"), JSON.stringify({ pid: process.pid }));
  if (mode === "tree" && existsSync(file("root-exit"))) process.exit(0);
  if (existsSync(file("release"))) {
    clearInterval(gate);
    clearTimeout(watchdog);
    if (mode !== "silent" && mode !== "terminal-wait" && !worker) emit("turn.completed");
    if (mode === "stderr") {
      for (let i = 0; i < 1000; i++) process.stderr.write(`synthetic stderr ${i}\n`);
    }
    process.exitCode = 0;
  }
}, 10);
