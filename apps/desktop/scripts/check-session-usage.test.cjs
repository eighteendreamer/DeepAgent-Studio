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
  assert.match(bar, /event\.type === "cost_recorded"[\s\S]{0,400}fetchSummary\(/);
});

// `cost_recorded` carries which spend landed. The bar may only settle its
// in-flight overlay for the run's own row: a compaction summary or session-note
// pass books a row while the run is still going, and clearing the overlay then
// made the bar lose totals the ledger had not received yet.
test("only the run's own ledger row settles the in-flight overlay", () => {
  const bar = read("components/SessionUsageBar.tsx");
  assert.match(
    bar,
    /fetchSummary\(String\(event\.origin \?\? "run"\) !== "side_call"\)/,
    "the pull must key off the event's origin, not off arrival order",
  );
  assert.match(
    bar,
    /async \(settleLive = true\)/,
    "mount and session-change pulls keep settling by default",
  );
  assert.match(bar, /if \(settleLive\) setLive\(EMPTY_LIVE\)/, "clearing must sit behind the flag");
});

// The origin crosses as snake_case JSON, so a renamed Rust variant would
// silently make every side-call row look like a run row again.
test("the cost origin vocabulary is shared with the backend", () => {
  const rust = fs.readFileSync(
    path.resolve(__dirname, "../../../crates/deepagent-runtime/src/events.rs"),
    "utf8"
  );
  const start = rust.indexOf("pub enum CostOrigin");
  assert.ok(start > -1, "CostOrigin must exist in the runtime event enum");
  const block = rust.slice(start, rust.indexOf("\n}", start));
  const variants = [...block.matchAll(/^\s{4}([A-Z][A-Za-z]*),$/gm)].map((match) => match[1]);
  assert.ok(
    variants.length >= 2,
    `expected a run row and a side-call row, found: ${variants.join(", ")}`,
  );
  const bar = read("components/SessionUsageBar.tsx");
  for (const variant of variants) {
    const wire = variant.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
    assert.match(
      bar,
      new RegExp(`"${wire}"`),
      `the bar must name "${wire}", or it will treat it as the run's own row`,
    );
  }
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

// The per-turn metrics line belongs to the TURN, not to a text block. A text
// block is reclassified from the answer surface into the collapsed process
// timeline every time the model starts another tool call after it
// (deriveTurnSections), which made the footer appear at each request's usage
// event and vanish again on the next tool_started.
test("the metrics footer is turn-level and renders exactly once", () => {
  const bubble = read("components/chat-timeline/MessageBubble.tsx");
  const turn = read("components/chat-timeline/MessageTurn.tsx");
  const grouped = read("components/chat-timeline/groupTurns.ts");

  assert.doesNotMatch(bubble, /<TurnFooter/, "a text block must not own the footer");
  assert.match(turn, /<TurnFooter[\s\S]{0,120}usage=\{turn\.usage\}/, "the footer must read turn metrics");
  assert.equal(
    (turn.match(/<TurnFooter/g) ?? []).length,
    1,
    "one turn must produce exactly one footer"
  );
  assert.match(grouped, /current\.usage = block\.usage/, "groupTurns no longer lifts usage");
  assert.match(turn, /previous\.usage === next\.usage/, "the memo would drop a usage-only change");
});

// Occupancy used to be recomputed by the frontend off `usage` events, which
// invented a 1M window whenever no snapshot had arrived and let two owners
// disagree on the same figure. The backend now re-publishes its own snapshot for
// every answered request, so the panel only renders what it is given.
test("context occupancy has one owner: the backend snapshot", () => {
  const app = read("App.tsx");
  const start = app.indexOf('case "usage": {');
  assert.ok(start > -1, "the usage event branch must exist");
  const usageBranch = app.slice(start, app.indexOf('case "', start + 10));
  assert.doesNotMatch(
    usageBranch,
    /setContextUsageByKey/,
    "the usage event must not write the occupancy map",
  );
  assert.doesNotMatch(app, /context_window:\s*1_000_000/, "the frontend must not invent a window");
  assert.doesNotMatch(app, /promptTokens\s*\/\s*1_000_000/, "nor a ratio off an invented window");
  assert.match(
    app,
    /case "context_usage"[\s\S]{0,300}setContextUsageByKey/,
    "the snapshot event stays the only writer",
  );
  const indicator = read("components/ContextCapacityIndicator.tsx");
  assert.match(
    indicator,
    /snapshot\.measured_prompt_tokens \?\? snapshot\.estimated_prompt_tokens/,
    "the measured request wins, the estimate covers the window before the first answer",
  );
});

// The popover listed the three biggest blocks against a measured total, so the
// rows could never add up to the headline it was showing.
test("the block breakdown truncates nothing and names its own gap", () => {
  const indicator = read("components/ContextCapacityIndicator.tsx");
  assert.doesNotMatch(indicator, /\.slice\(0, 3\)/, "dropping rows made them disagree with the total");
  assert.match(indicator, /其他\/未归类/, "context the rows do not name gets a row");
  assert.match(indicator, /装配期估算/, "the rows are an estimate, and must say so");
});
