const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const sourceRoot = path.resolve(__dirname, "../src");

function read(relative) {
  return fs.readFileSync(path.join(sourceRoot, relative), "utf8");
}

// The desktop publishes `chat://event` as a run envelope `{run_id, payload}`.
// Reading `event.payload.type` off that envelope is `undefined`, which silently
// disabled every auto-refresh the usage bar had.
test("usage bar unwraps the run envelope instead of reading the envelope tag", () => {
  const bar = read("components/SessionUsageBar.tsx");
  assert.match(bar, /unwrapRuntimeEvent\(/, "the bar must normalize chat://event payloads");
  assert.doesNotMatch(bar, /\.payload\.type/, "the envelope has no `type` field to test");
});

test("envelope unwrap is implemented once and handles both published shapes", () => {
  const api = read("api.ts");
  const helper = api.slice(
    api.indexOf("export function unwrapRuntimeEvent"),
    api.indexOf("export async function sandboxieStatus")
  );
  assert.ok(helper.length > 80, "unwrapRuntimeEvent must exist in api.ts");
  assert.match(helper, /\.payload\b/, "handles the run envelope shape");
  assert.match(helper, /typeof bare\.type === "string"/, "handles the bare-event shape");
  assert.doesNotMatch(read("components/ChatView.tsx"), /\.payload\.type/);
});

test("usage refresh is event-driven, not a polling timer", () => {
  const bar = read("components/SessionUsageBar.tsx");
  assert.doesNotMatch(bar, /setTimeout\(fetchSummary/, "a fixed delay races the ledger write");
  assert.match(bar, /event\.type === "cost_recorded"[\s\S]{0,240}fetchSummary\(\)/);
});

test("switching sessions drops the previous session's numbers", () => {
  const bar = read("components/SessionUsageBar.tsx");
  assert.match(
    bar,
    /setSummary\(null\)[\s\S]{0,160}\}, \[sessionId\]\);/,
    "a session change must clear the settled summary synchronously"
  );
  assert.match(bar, /requested !== generation\.current/, "stale replies must be discarded");
});

test("live usage overlay is gated on the visible session running", () => {
  const bar = read("components/SessionUsageBar.tsx");
  assert.match(bar, /event\.type === "usage" && busyRef\.current/);
  const chat = read("components/ChatView.tsx");
  assert.match(chat, /<SessionUsageBar sessionId=\{sessionId \?\? null\} busy=\{busy\} \/>/);
});
