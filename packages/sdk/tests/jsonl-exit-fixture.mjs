import { existsSync, writeFileSync } from "node:fs";

const [mode, recordPath, releasePath, ...args] = process.argv.slice(2);
if (args[0] !== "run" || args[1] !== "--json" || args[2] !== "audit prompt") {
  throw new Error("fixture did not receive the production runJsonl argument shape");
}

process.on("exit", (code) => {
  writeFileSync(recordPath, JSON.stringify({ pid: process.pid, code }));
});

if (mode !== "empty-failure") {
  process.stdout.write(`${JSON.stringify({
    type: "item.updated", threadId: "audit-thread", turnId: "audit-turn",
    item: { kind: "content_delta", text: "partial canary" }
  })}\n`);
}

if (mode === "success") {
  process.stdout.write(`${JSON.stringify({
    type: "turn.completed", threadId: "audit-thread", turnId: "audit-turn",
    message: "done"
  })}\n`);
} else if (mode === "early-eof") {
  // The parent releases this gate only after observing the SDK result.
  // A hard bound also makes an interrupted test self-cleaning.
  const watchdog = setTimeout(() => process.exit(23), 5000);
  const gate = setInterval(() => {
    if (existsSync(releasePath)) {
      clearInterval(gate);
      clearTimeout(watchdog);
      process.exit(23);
    }
  }, 10);
  process.stdout.end(() => {
    writeFileSync(`${recordPath}.ready`, JSON.stringify({ pid: process.pid }));
  });
} else if (mode === "partial-failure" || mode === "empty-failure") {
  process.stderr.write("audit fixture failure: code 23\n");
  process.exitCode = 23;
} else {
  throw new Error(`unknown fixture mode: ${mode}`);
}
