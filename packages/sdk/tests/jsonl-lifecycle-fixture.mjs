import { existsSync, writeFileSync } from "node:fs";
import path from "node:path";

const [mode, root, ...args] = process.argv.slice(2);
if (args.join(" ") !== "run --json lifecycle audit") {
  throw new Error("unexpected SDK CLI arguments");
}
writeFileSync(path.join(root, "ready.json"), JSON.stringify({ pid: process.pid }));

const watchdog = setTimeout(() => process.exit(24), 5000);
const gate = setInterval(() => {
  if (existsSync(path.join(root, "probe"))) {
    // This file is created only after the SDK's lifecycle operation returned.
    writeFileSync(path.join(root, "probe-ack.json"), JSON.stringify({ pid: process.pid }));
  }
  if (existsSync(path.join(root, "release"))) {
    clearInterval(gate);
    clearTimeout(watchdog);
    if (mode === "normal") {
      process.stdout.write(`${JSON.stringify({
        type: "turn.completed", threadId: "lifecycle-thread", turnId: "lifecycle-turn",
        message: "done"
      })}\n`);
    }
    process.exitCode = 0;
  }
}, 10);

if (mode === "malformed") {
  process.stdout.write("not-json\n");
} else {
  process.stdout.write(`${JSON.stringify({
    type: "item.updated", threadId: "lifecycle-thread", turnId: "lifecycle-turn",
    item: { kind: "content_delta", text: "lifecycle canary" }
  })}\n`);
}
