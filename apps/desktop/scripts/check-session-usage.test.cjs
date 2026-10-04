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

// A brand-new chat used to inherit the previously selected session as its
// identity until `session_registered` arrived, which made the usage bar read the
// wrong ledger and let a late event move the view out from under the user.
test("a pending chat frame has no session identity", () => {
  const app = read("App.tsx");
  assert.match(
    app,
    /const chatSessionId =\s*\n?\s*activePendingRunKey !== null && chatFrameKey === `chat:\$\{activePendingRunKey\}`\s*\n\s*\?\s*null\s*\n\s*: activeId;/,
    "the frame must report null while its session is not decided yet"
  );
  assert.match(app, /sessionId=\{chatSessionId\}/, "the chat frame must use the derived id");
  assert.doesNotMatch(app, /sessionId=\{activeId\}/, "raw activeId must not reach the frame");
  assert.doesNotMatch(app, /sessionKey=/, "the unused sessionKey prop must not come back");
});

test("session adoption cannot hijack a view the user navigated away from", () => {
  const app = read("App.tsx");
  const adopt = app.slice(app.indexOf("const adoptSession"), app.indexOf("case \"reasoning_delta\""));
  assert.ok(adopt.length > 400, "adoptSession must exist");
  assert.match(adopt, /const stillOnSubmittingFrame = activePendingRunKeyRef\.current === pendingKey;/);
  const moveIndex = adopt.indexOf("stillOnSubmittingFrame)");
  const setActiveIndex = adopt.indexOf("setActiveId(sid)");
  assert.ok(moveIndex > -1 && setActiveIndex > moveIndex, "setActiveId must sit behind the guard");
  assert.match(
    app,
    /case "session_registered": \{[\s\S]{0,400}adoptSession\(String\(event\.session_id \?\? ""\)\);/,
    "the event path must reuse the single adoption implementation"
  );
});

test("the backend decides the session before spawning the run", () => {
  const rust = fs.readFileSync(
    path.resolve(sourceRoot, "../src-tauri/src/lib.rs"),
    "utf8"
  );
  const command = rust.slice(rust.indexOf("fn start_chat_v2("), rust.indexOf("/// Canvas settings payload"));
  assert.ok(command.length > 400, "start_chat_v2 must be found");
  assert.match(command, /\.create_session\(env_mode\.as_deref\(\)\)/, "identity must be minted here");
  assert.ok(
    command.indexOf(".create_session(") < command.indexOf("state.rt.spawn"),
    "the session must exist before the run task starts"
  );
});
