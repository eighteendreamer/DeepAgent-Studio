// Gate: a compaction boundary or an injected-memory batch must survive to the
// rendered timeline.
//
// Why this exists: the fact starts life as a durable kernel event
// (`ContextCompacted` with a summary, `MemoriesInjected`), becomes a
// `ConversationPartDto` in Rust, then a `MessagePart` in TypeScript, then a
// `ChatBlock`, then a row. Each hop is a separate file with its own vocabulary,
// and a missing arm anywhere degrades silently: the session reloads and the
// marker is just... not there (the exact complaint this whole track exists to
// kill). So the chain is asserted end to end here, both directions:
//   - every hop declares the kinds the previous hop emits;
//   - internal bookkeeping records stay invisible (a history snip is not a
//     conversation boundary and must not spam the transcript).
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const ROOT = path.resolve(__dirname, "..");
const read = (relative) =>
  fs.readFileSync(path.resolve(ROOT, relative), "utf8");

const SERVICE_RS = read("../../crates/deepagent-app-core/src/service.rs");
const DTO_RS = read("../../crates/deepagent-app-core/src/dto.rs");
const API_TS = read("src/api.ts");
const TYPES_TS = read("src/types.ts");
const MAPPER_TS = read("src/components/chat-timeline/chatMessageMapper.ts");
const SECTIONS_TS = read("src/components/chat-timeline/deriveTurnSections.ts");
const BLOCK_TYPES_TS = read("src/components/chat-timeline/timelineTypes.ts");
const TURN_TSX = read("src/components/chat-timeline/MessageTurn.tsx");
const MARKER_TSX = read("src/components/chat-timeline/SessionMarkerRow.tsx");
const APP_TSX = read("src/App.tsx");

test("the kernel parts exist on the Rust DTO", () => {
  assert.match(DTO_RS, /CompactBoundary \{/, "ConversationPartDto lost CompactBoundary");
  assert.match(DTO_RS, /Memories \{/, "ConversationPartDto lost Memories");
});

test("session_conversation projects a boundary only when it is durable", () => {
  const projection = /EventPayload::ContextCompacted \{[\s\S]*?summary: Some\(summary\),[\s\S]*?\} => \{/;
  assert.match(SERVICE_RS, projection, "only summary-bearing compaction records may render");
  assert.match(
    SERVICE_RS,
    /ConversationPartDto::CompactBoundary \{[\s\S]*?summary: summary\.clone\(\),/,
    "the boundary row would have nothing to show",
  );
  assert.match(DTO_RS, /CompactBoundary \{[\s\S]*?summary: String,/, "DTO dropped the summary");
  assert.match(SERVICE_RS, /EventPayload::MemoriesInjected \{/, "memories are not replayed");
  // Regression cover for the two claims above.
  assert.match(
    SERVICE_RS,
    /fn durable_compaction_boundary_is_replayed_as_its_own_row/,
    "no test pins the boundary row",
  );
  assert.match(
    SERVICE_RS,
    /fn metrics_only_compaction_records_render_no_row/,
    "nothing stops internal records from becoming rows",
  );
});

test("the live event names the kernel emits are in the frontend union", () => {
  // The labels come from RuntimeEvent::label(); adding an event without a union
  // member makes the switch below silently never match.
  assert.match(API_TS, /"context_compacted"/, "api.ts cannot receive context_compacted");
  assert.match(API_TS, /"relevant_memories_injected"/, "api.ts cannot receive the memory event");
  assert.match(APP_TSX, /case "context_compacted"/, "the live boundary is dropped on arrival");
  assert.match(APP_TSX, /case "relevant_memories_injected"/, "the live injection is dropped");
  assert.match(APP_TSX, /strategy === "history_snip"/, "snips would clutter the transcript");
});

test("markers survive a model-attempt retry", () => {
  // A reactive compaction happens *inside* an attempt; the attempt reset that
  // follows rewinds the turn to its pre-attempt parts. Without carrying the
  // marker forward the row vanishes until the session is reloaded.
  assert.match(
    APP_TSX,
    /case "model_attempt_reset"[\s\S]{0,900}part\.kind === "compact_boundary"/,
    "attempt reset rewinds away the compaction/memory marker",
  );
});

test("every part kind has a TypeScript shape and a block", () => {
  for (const kind of ["compact_boundary", "memories"]) {
    assert.match(TYPES_TS, new RegExp(`kind: "${kind}"`), `types.ts has no ${kind} part`);
    assert.match(
      MAPPER_TS,
      new RegExp(`part\\.kind === "${kind}"`),
      `the mapper drops ${kind} parts`,
    );
  }
  // Conversation replay mirrors the DTO's snake_case field names.
  assert.match(TYPES_TS, /tokens_before: number/, "replayed boundary fields are not typed");
  assert.match(
    TYPES_TS,
    /kind: "compact_boundary";\s*[\s\S]{0,160}?\bsummary: string;/,
    "neither TypeScript part shape carries the summary",
  );
});

test("the boundary row says what compacted and shows what it became", () => {
  // The plan's two facts a user must be able to read off the line: which path
  // supplied the summary, and the summary text itself.
  assert.match(MARKER_TSX, /STRATEGY_LABELS\[block\.strategy\]/, "strategy is never shown");
  assert.match(MARKER_TSX, /session_memory: /, "the session-note strategy has no label");
  assert.match(MARKER_TSX, /查看摘要/, "the summary cannot be opened");
  assert.match(MARKER_TSX, /\{summary\}/, "the opened block shows nothing but a title");
});

// The strategy label is a cross-language wire value: Rust writes it, the desktop
// turns it into Chinese. Without one source, a new producer string reaches the
// user as a raw token and nothing fails.
const STRATEGY_RS = read("../../crates/deepagent-core/src/compaction_strategy.rs");

function rustStrategies() {
  const values = new Map();
  for (const [, name, value] of STRATEGY_RS.matchAll(/pub const (\w+): &str = "([^"]+)";/g)) {
    values.set(name, value);
  }
  const list = STRATEGY_RS.match(/pub const ALL: &\[\&str\] = \&\[([\s\S]*?)\];/);
  assert.ok(list, "compaction_strategy.rs lost its ALL list");
  return [...list[1].matchAll(/\b([A-Z][A-Z0-9_]+)\b/g)]
    .map(([, name]) => {
      assert.ok(values.has(name), `ALL references ${name}, which has no label`);
      return values.get(name);
    });
}

test("the UI strategy labels and the Rust vocabulary cover each other", () => {
  const strategies = rustStrategies();
  assert.ok(strategies.length >= 8, `suspiciously small vocabulary: ${strategies}`);

  const labelled = new Set(
    [...MARKER_TSX.matchAll(/^\s{2}([a-z_0-9]+): /gm)].map(([, key]) => key)
  );
  // `history_snip` is internal bookkeeping: App.tsx drops that row on purpose,
  // so it needs no Chinese label. Anything else must have one.
  const missing = strategies.filter(
    (value) => value !== "history_snip" && !labelled.has(value)
  );
  assert.deepEqual(missing, [], "the UI would show these strategies raw");

  // And the other direction: a label with no producer is dead UI.
  const invented = [...labelled].filter((key) => !strategies.includes(key));
  assert.deepEqual(invented, [], "the UI labels strategies Rust never writes");
});

test("markers render as their own quiet rows, not process steps", () => {
  assert.match(
    SECTIONS_TS,
    /block\.kind === "compaction" \|\| block\.kind === "memories"[\s\S]{0,120}markerBlocks\.push/,
    "markers must leave the collapsed process list",
  );
  assert.match(TURN_TSX, /markerBlocks\.map/, "MessageTurn never renders markers");
  assert.match(MARKER_TSX, /formatTokens\(/, "the boundary row should show what was reclaimed");
  // Project rule: icons are SVG components, never glyph characters.
  assert.match(MARKER_TSX, /from "lucide-react"/, "marker icons must come from lucide");
  assert.doesNotMatch(MARKER_TSX, /[\u{1F300}-\u{1FAFF}\u{2600}-\u{27BF}]/u, "emoji icon in the row");
});

test("no durable event disappears from the transcript without a decision", () => {
  // Phase 4 §7.1's real requirement: an unmapped durable payload is how "the
  // content is gone after a reload" starts. Every variant must be either
  // projected by session_conversation or declared deliberately invisible HERE,
  // with a reason — so a new event cannot land in a silent `_ => {}`.
  const eventRs = read("../../crates/deepagent-core/src/event.rs");
  const body = eventRs.slice(eventRs.indexOf("pub enum EventPayload"));
  const variants = [...body.matchAll(/^ {4}([A-Z][A-Za-z0-9]*) \{/gm)].map((found) => found[1]);
  assert.ok(variants.length >= 17, `parsed only ${variants.length} EventPayload variants`);

  const deliberatelyInvisible = {
    SessionStarted: "session metadata; the transcript opens with the first message",
    SessionEnded: "lifecycle marker for the session list, not a row",
    TaskCreated: "task bookkeeping; the plan panel renders it",
    TaskStateChanged: "same — transitions belong to the plan panel's vocabulary",
    RunTerminal: "run outcome lives in the usage footer and the run store",
    ContextSnipped: "internal history edit; a snip is not a conversation boundary",
    ContextSurfacePatched: "internal model Surface replacement; the raw tool transcript remains unchanged",
    MemoryEntryWritten: "vault audit trail, surfaced in the knowledge panel",
    Note: "consolidation result for the observation timeline, not the chat",
    McpInstructionsDelta: "bookkeeping about which servers announced",
    ToolsDiscovered: "deferred tool catalog bookkeeping",
  };
  const unmapped = [];
  for (const variant of variants) {
    if (SERVICE_RS.includes(`EventPayload::${variant}`)) continue;
    if (!deliberatelyInvisible[variant]) unmapped.push(variant);
  }
  assert.deepEqual(unmapped, [], "these durable events have neither a projection nor a reason");

  assert.match(
    SERVICE_RS,
    /other => \{[\s\S]{0,260}tracing::debug!\([\s\S]{0,160}no conversation projection/,
    "unmapped durable events must be logged, not silently dropped",
  );
});

test("the memory row names the entries it brought in, not only a count", () => {
  // §7.2: "注入了哪几条". The ids ride the durable event and the DTO already; the
  // failure mode here is dropping them at the block hop, which leaves a number
  // where a name belongs.
  assert.match(
    BLOCK_TYPES_TS,
    /kind: "memories"[\s\S]{0,160}ids: string\[\]/,
    "the memories block carries no entry ids",
  );
  assert.match(MAPPER_TS, /ids: part\.ids \?\? \[\]/, "the mapper drops the ids");
  assert.match(MARKER_TSX, /ids\.map\(/, "SessionMarkerRow never renders them");
});
