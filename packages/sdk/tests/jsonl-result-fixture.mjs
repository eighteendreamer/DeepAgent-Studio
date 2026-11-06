// Only synthetic events and files, in the caller's private temporary root.
import { existsSync, writeFileSync } from "node:fs";
import path from "node:path";

const [root, mode, run, json, id] = process.argv.slice(2);
if (run !== "run" || json !== "--json" || !/^[a-z-]+$/u.test(id)) {
  throw new Error("unexpected JSONL result fixture arguments");
}
const file = (suffix) => path.join(root, `${id}.${suffix}`);
const event = (type) => process.stdout.write(JSON.stringify({
  type, threadId: "synthetic-thread", turnId: id,
  item: { kind: "content_delta", text: "synthetic result control" },
  message: "synthetic completion", reason: "synthetic business failure"
}) + "\n");
process.on("exit", (code) => writeFileSync(file("exit.json"), JSON.stringify({ pid: process.pid, code })));
writeFileSync(file("ready.json"), JSON.stringify({ pid: process.pid }));

if (mode === "malformed") process.stdout.write("{invalid-json\n");
else if (mode !== "empty") event(mode === "partial" || mode === "signal" ? "item.updated" : mode);

if (mode === "signal") {
  // Only the test-owned parent may terminate this silent process.
  setTimeout(() => process.exit(24), 8000);
} else {
  const watchdog = setTimeout(() => process.exit(24), 8000);
  const gate = setInterval(() => {
    if (!existsSync(file("release"))) return;
    clearInterval(gate);
    clearTimeout(watchdog);
    // Deliberately no final newline: status validation must wait for stderr drain.
    process.stderr.write("synthetic-private-stderr-canary");
    process.exitCode = id.startsWith("zero-") ? 0 : 23;
  }, 10);
}
