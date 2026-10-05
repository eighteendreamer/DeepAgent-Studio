// Gate: the desktop's knowledge-kind vocabularies must cover every kind the
// Rust kernel can emit.
//
// Why this exists: `EntryKind` is an exhaustive Rust enum, but the frontend
// stores `kind` as a plain string and keys its filter chips, badge visuals,
// legend dots, graph colors and i18n labels off hard-coded maps. Adding a kind
// in Rust therefore compiles and runs fine while the new entries silently get no
// filter chip, no color and an untranslated label. That failure is invisible
// until a user notices entries missing from a panel, so it is checked here.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const ROOT = path.resolve(__dirname, "..");
const ENTRY_RS = path.resolve(ROOT, "../../crates/deepagent-knowledge/src/entry.rs");
const VIEW_TSX = path.resolve(ROOT, "src/components/KnowledgeView.tsx");
const GRAPH_TSX = path.resolve(ROOT, "src/components/KnowledgeGraph.tsx");
const ZH = path.resolve(ROOT, "src/locales/zh.json");
const EN = path.resolve(ROOT, "src/locales/en.json");

/** Every `EntryKind` label the kernel can write to a vault file. */
function rustKindLabels() {
  const source = fs.readFileSync(ENTRY_RS, "utf8");
  const enumBody = /enum EntryKind \{([\s\S]*?)\n\}/.exec(source);
  assert.ok(enumBody, "could not locate `enum EntryKind` in entry.rs");
  const variants = [...enumBody[1].matchAll(/^\s{4}([A-Z][A-Za-z]+),$/gm)].map((m) => m[1]);
  assert.ok(variants.length >= 5, `expected the full variant list, got ${variants}`);

  const labels = variants.map((variant) => {
    const arm = new RegExp(`EntryKind::${variant} => "([a-z-]+)"`).exec(source);
    assert.ok(arm, `EntryKind::${variant} has no label() arm in entry.rs`);
    return arm[1];
  });
  assert.equal(new Set(labels).size, labels.length, "kind labels must be unique");
  return labels;
}

/** Keys of a `const NAME: Record<string, ...> = { ... }` literal. */
function recordKeys(source, name) {
  const body = new RegExp(`const ${name}[^=]*= \\{([\\s\\S]*?)\\n\\};`).exec(source);
  assert.ok(body, `could not locate record \`${name}\``);
  return [...body[1].matchAll(/^\s*([a-z-]+):/gm)].map((m) => m[1]);
}

/** Members of a `const NAME = [...] as const` literal. */
function arrayMembers(source, name) {
  const body = new RegExp(`const ${name} = \\[([^\\]]*)\\]`).exec(source);
  assert.ok(body, `could not locate array \`${name}\``);
  return [...body[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

function kindSection(file) {
  const json = JSON.parse(fs.readFileSync(file, "utf8"));
  const section = json.knowledgeView && json.knowledgeView.kind;
  assert.ok(section, `${path.basename(file)} has no knowledgeView.kind section`);
  return section;
}

test("knowledge kind vocabularies cover every Rust EntryKind", () => {
  const labels = rustKindLabels();
  const view = fs.readFileSync(VIEW_TSX, "utf8");
  const graph = fs.readFileSync(GRAPH_TSX, "utf8");

  const kinds = arrayMembers(view, "KINDS");
  const visuals = recordKeys(view, "map"); // visualFor()'s per-kind icon+color table
  const dots = recordKeys(view, "KIND_DOT");
  const colors = recordKeys(graph, "KIND_COLOR");
  const zh = kindSection(ZH);
  const en = kindSection(EN);

  for (const label of labels) {
    // A missing filter chip means the user cannot list those entries at all.
    assert.ok(kinds.includes(label), `KnowledgeView KINDS is missing "${label}"`);
    // Missing visuals fall back to the generic note badge — wrong color, but
    // silent, so pin them here instead of letting the fallback absorb them.
    assert.ok(visuals.includes(label), `visualFor() has no entry for "${label}"`);
    assert.ok(dots.includes(label), `KIND_DOT has no entry for "${label}"`);
    assert.ok(colors.includes(label), `KnowledgeGraph KIND_COLOR has no entry for "${label}"`);
    assert.ok(zh[label], `zh.json knowledgeView.kind is missing "${label}"`);
    assert.ok(en[label], `en.json knowledgeView.kind is missing "${label}"`);
  }

  // The reverse direction matters too: a stale frontend entry (e.g. a removed
  // kind) would render a filter chip that can never match anything.
  for (const kind of kinds) {
    assert.ok(labels.includes(kind), `KnowledgeView offers unknown kind "${kind}"`);
  }
});
