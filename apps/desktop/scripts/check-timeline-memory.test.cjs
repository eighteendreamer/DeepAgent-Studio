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
const TURN_TSX = read("src/components/chat-timeline/MessageTurn.tsx");
const MARKER_TSX = read("src/components/chat-timeline/SessionMarkerRow.tsx");
const APP_TSX = read("src/App.tsx");

test("the kernel parts exist on the Rust DTO", () => {
  assert.match(DTO_RS, /CompactBoundary \{/, "ConversationPartDto lost CompactBoundary");
  assert.match(DTO_RS, /Memories \{/, "ConversationPartDto lost Memories");
});

test("session_conversation projects a boundary only when it is durable", () => {
  const projection = /EventPayload::ContextCompacted \{[\s\S]*?summary: Some\(_\),[\s\S]*?\} => \{/;
  assert.match(SERVICE_RS, projection, "only summary-bearing compaction records may render");
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
