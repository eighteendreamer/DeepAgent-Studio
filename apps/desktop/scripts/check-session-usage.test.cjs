const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const sourceRoot = path.resolve(__dirname, "../src");

test("auxiliary call cost refreshes session totals without replacing main-turn cost", () => {
  const source = read("App.tsx");
  const body = source.slice(source.indexOf('case "cost_recorded":'), source.indexOf('case "run_completed":', source.indexOf('case "cost_recorded":')));
  const dispatch = new Function("event", "updateTranscript", `switch (event.type) { ${body} }`);
  const original = [{ role: "assistant", usage: { prompt: 1000, costYuan: 0.2 } }];
  let updates = 0;
  let state = original;
  const update = (fn) => { updates++; state = fn(state); };
  dispatch({ type: "cost_recorded", origin: "side_call", cost_yuan: 0.1 }, update);
  assert.equal(updates, 0);
  assert.equal(state, original);
  for (const origin of ["run", undefined]) {
    dispatch({ type: "cost_recorded", origin, cost_yuan: 0.3 }, update);
    assert.deepEqual(state[0].usage, { prompt: 1000, costYuan: 0.3 });
  }
});

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

test("the backend admits the run before spawning its execution task", () => {
  const rust = fs.readFileSync(
    path.resolve(sourceRoot, "../src-tauri/src/lib.rs"),
    "utf8"
  );
  const command = rust.slice(rust.indexOf("fn start_chat_v2("), rust.indexOf("/// Canvas settings payload"));
  assert.ok(command.length > 400, "start_chat_v2 must be found");
  assert.match(
    command,
    /\.create_session\(admission_mode\.as_deref\(\)\)/,
    "a new session must be created before admission"
  );
  assert.match(command, /\.admit_run_input\(&admission_run_id, &id, &admission_input\)/, "lifecycle controls must be recognized before admission");
  assert.ok(
    command.indexOf(".admit_run_input(") < command.indexOf("state.rt.spawn"),
    "the run record must exist before the execution task starts"
  );
  assert.match(command, /\.run_prepared\(prepared, &prompt, on_event, on_approval\)/, "the exact admitted control must be consumed without legacy re-admission");
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
    /snapshot\.projected_request_tokens \?\? snapshot\.measured_prompt_tokens \?\? snapshot\.estimated_prompt_tokens/,
    "the next request projection wins; legacy snapshots keep their fallback",
  );
});

// The popover listed the three biggest blocks against a measured total, so the
// rows could never add up to the headline it was showing.
test("the block breakdown truncates nothing and names its own gap", () => {
  const indicator = read("components/ContextCapacityIndicator.tsx");
  assert.doesNotMatch(indicator, /\.slice\(0, 3\)/, "dropping rows made them disagree with the total");
  assert.match(indicator, /其他\/未归类/, "context the rows do not name gets a row");
  assert.match(indicator, /上下文组成（估算）/, "the rows are an estimate, and must say so");
});

// H-163 crosses the existing UI contract; exercise its real mapper, event
// handlers and footer, rather than a second implementation of those rules.
function productionUsageNodes() {
  const ts = require("typescript");
  const ast = ts.createSourceFile("App.tsx", read("App.tsx"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const found = {};
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name?.text === "mapConversationToChatMessages") found.mapper = node;
    if (ts.isFunctionDeclaration(node) && node.name?.text === "mapUsageBuckets") found.usageBuckets = node;
    if (ts.isVariableDeclaration(node) && node.name.getText(ast) === "addUsage") found.live = node;
    if (ts.isCaseClause(node) && node.expression.getText(ast) === '"cost_recorded"') found.cost = node;
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.deepEqual(Object.keys(found).sort(), ["cost", "live", "mapper", "usageBuckets"]);
  const compile = (code) => ts.transpileModule(code, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  return { ts, ast, found, compile };
}

function loadUsageComponent(relative, imports = {}) {
  const ts = require("typescript");
  const Module = require("node:module");
  const filename = path.join(sourceRoot, relative);
  const loaded = new Module(filename, module);
  loaded.filename = filename;
  loaded.paths = Module._nodeModulePaths(path.dirname(filename));
  const original = loaded.require.bind(loaded);
  loaded.require = (name) => Object.hasOwn(imports, name) ? imports[name] : original(name);
  loaded._compile(ts.transpileModule(read(relative), {
    fileName: filename,
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
  }).outputText, filename);
  return loaded.exports;
}

test("workflow persisted usage reaches the production mapper and rendered footer", () => {
  const { ast, found, compile } = productionUsageNodes();
  const mapper = new Function(
    `${compile(found.usageBuckets.getText(ast))};${compile(found.mapper.getText(ast))}; return mapConversationToChatMessages;`,
  )();
  // Legacy payloads still map safely; current payloads carry explicit
  // turn_total/last_request buckets so live and replay share one footer.
  const [message] = mapper([{
    id: "workflow", role: "assistant", content: "answer", parts: [],
    usage: { prompt_tokens: 50, completion_tokens: 30, reasoning_tokens: 17,
      total_tokens: 60, prompt_cache_hit_tokens: 20, prompt_cache_miss_tokens: 30,
      duration_ms: 12, cost_yuan: 0.000310 },
  }]);
  assert.deepEqual(message.usage, {
    promptTokens: 50, completionTokens: 30, reasoningTokens: 17, totalTokens: 60,
    cacheHitTokens: 20, cacheMissTokens: 30,
    lastRequest: {
      promptTokens: 50, completionTokens: 30, reasoningTokens: 17, totalTokens: 60,
      cacheHitTokens: 20, cacheMissTokens: 30,
    },
    costYuan: 0.000310,
  });
  assert.equal(message.runMs, 12);
  const React = require("react");
  const { renderToStaticMarkup } = require("react-dom/server");
  const format = loadUsageComponent("components/chat-timeline/format.ts");
  const { TurnFooter } = loadUsageComponent("components/chat-timeline/TurnFooter.tsx", {
    "../ui/HoverInfo": { HoverInfo: ({ children }) => children },
    "./EditedFilesList": { EditedFilesList: () => null },
    "./format": format,
  });
  const render = (usage) => renderToStaticMarkup(React.createElement(TurnFooter, {
    usage, totalMs: message.runMs, answer: message.content,
  })).replace(/<[^>]*>/g, "");
  const text = render(message.usage);
  for (const label of ["60 tokens", "输入 50", "输出 30", "缓存命中 20", "缓存未命中 30", "￥0.000310"]) {
    assert.ok(text.includes(label), `rendered workflow metric missing: ${label}`);
  }
  assert.ok(render({ ...message.usage, costYuan: undefined }).includes("￥--"), "unknown pricing must not display a measured zero");
});

test("one aggregate workflow cost receipt preserves the production turn projection", () => {
  const { ts, ast, found, compile } = productionUsageNodes();
  let messages = [{ role: "assistant", content: "answer" }];
  const updateTranscript = (update) => { messages = update(messages); };
  const addUsage = new Function("updateTranscript", `${compile(`const ${found.live.getText(ast)};`)}; return addUsage;`)(updateTranscript);
  addUsage({ prompt: 100, completion: 20, reasoning: 12, cacheHit: 80, cacheMiss: 20 });
  addUsage({ prompt: 50, completion: 10, reasoning: 5, cacheHit: 0, cacheMiss: 50 });
  const before = { ...messages[0].usage };
  assert.equal(before.promptTokens, 150);
  assert.equal(before.completionTokens, 30);
  assert.equal(before.reasoningTokens, 17);
  assert.equal(before.totalTokens, 180);
  assert.equal(before.cacheHitTokens, 80);
  assert.equal(before.cacheMissTokens, 70);
  // Execute the actual cost case without its switch-only break statement.
  const block = found.cost.statements[0];
  assert.ok(ts.isBlock(block));
  const body = block.statements.filter((node) => !ts.isBreakStatement(node)).map((node) => node.getText(ast)).join("\n");
  new Function("updateTranscript", "event", compile(body))(updateTranscript, {
    type: "cost_recorded", model: "workflow", origin: "run", cost_yuan: 0.000310,
  });
  assert.deepEqual(messages[0].usage, { ...before, costYuan: 0.000310 });
});
