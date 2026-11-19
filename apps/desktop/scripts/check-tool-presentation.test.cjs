const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");
const filename = path.resolve(__dirname, "../src/components/chat-timeline/toolPresentation.ts");
const moduleUnderTest = { exports: {} };
vm.runInNewContext(ts.transpileModule(fs.readFileSync(filename, "utf8"), {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText, { module: moduleUnderTest, exports: moduleUnderTest.exports });
const { presentTool } = moduleUnderTest.exports;
const tool = (fields) => ({ call_id: "one", name: "skill", args: "{}", status: "ok", ...fields });

test("the reported skill result produces a name, never a JSON or instruction title", () => {
  const result = presentTool(tool({ output: { id: "using-superpowers", name: "using-superpowers", body: "<SUBAGENT-STOP>private instructions</SUBAGENT-STOP>" }, detail: '{"body":"private instructions"}' }));
  assert.equal(result.kind, "skill");
  assert.equal(result.target, "using-superpowers");
  assert.equal(result.description.includes("private instructions"), false);
  assert.match(result.skillBody, /private instructions/);
  assert.match(result.raw, /private instructions/);
});

test("read_file preserves its actual offset and strips only real line prefixes", () => {
  const result = presentTool(tool({ name: "read_file", args: '{"path":"src/main.ts","offset":118}', output: { path: "src/main.ts", content: "   118\tconst a = 1;\n   119\t\treturn a;", offset: 118, returned_lines: 2, truncated: true, next_offset: 120 } }));
  assert.equal(result.read.startLine, 118);
  assert.equal(result.read.text, "const a = 1;\n\treturn a;");
  assert.equal(result.read.truncated, true);
  assert.match(result.description, /118–119/);
  const cached = presentTool(tool({ name: "read_file", output: { content: "FILE_UNCHANGED", unchanged_stub: true } }));
  assert.equal(cached.read, undefined);
  assert.match(cached.description, /未变化/);
});

test("stringified command results preserve stderr and nonzero exit codes", () => {
  const result = presentTool(tool({ name: "bash", status: "error", args: '{"command":"pnpm build"}', output: JSON.stringify({ exit_code: 2, stdout: "building", stderr: "type error", shell: "powershell" }) }));
  assert.equal(result.command.command, "pnpm build");
  assert.equal(result.command.exitCode, 2);
  assert.equal(result.command.stderr, "type error");
});

test("successful mutation metadata owns the diff, never a live workspace Git diff", () => {
  const result = presentTool(tool({ name: "edit_file", args: '{"old":"wrong","new":"wrong"}', meta: { client_metadata: { file_change: { version: 1, before: "before\n", after: "after\n", start_line: 42 } } } }));
  assert.equal(result.changes[0].before, "before\n");
  assert.equal(result.changes[0].startLine, 42);
  assert.equal(result.changes[0].scope, "本次修改");
  const old = presentTool(tool({ name: "edit_file", args: '{"path":"a.ts","old":"a","new":"b"}' }));
  assert.equal(old.changes[0].scope, "替换片段");
  assert.equal(old.changes[0].startLine, undefined);
  const failed = presentTool(tool({ name: "edit_file", status: "error", args: '{"old":"a","new":"b"}', output: { error: "stale read" } }));
  assert.equal(failed.changes.length, 0);
});

test("directory reads and unknown or failed tools retain usable details", () => {
  const directory = presentTool(tool({ name: "list_dir", toolKind: "file_read", output: { path: "src", entries: [{ name: "a.ts" }] } }));
  assert.equal(directory.read, undefined);
  assert.equal(directory.entries[0].name, "a.ts");
  assert.match(directory.raw, /a.ts/);
  const fallback = presentTool(tool({ name: "custom_tool", detail: '{"error":"denied"}' }));
  assert.equal(fallback.target.includes("{"), false);
  assert.match(fallback.raw, /denied/);
});

// Exercise the production section and tool callbacks together: the reported
// regression only appears when a tool is wrapped by its containing section.
function timelineFixture(tools) {
  const state = new Map();
  const cache = new Map();
  const slots = ["CodeBlock", "CodeBlockContent", "CodeBlockHeader", "CodeBlockTitle",
    "CodeBlockActions", "CodeBlockCopyButton"];
  const dependencies = {
    react: { useMemo: fn => fn() },
    "lucide-react": new Proxy({}, { get: (_, key) => `icon-${key}` }),
    "../shadcn/button": { Button: "button" },
    "../ui/HoverInfo": { HoverInfo: ({ children }) => children },
    "../shadcn/code-block": Object.fromEntries(slots.map(slot => [slot, slot])),
    "../MarkdownText": { MarkdownText: ({ text }) => text },
    "./CollapseBlurPanel": { CollapseBlurPanel: ({ open, children }) => open ? children : null,
      CollapseChevron: "chevron" },
    "./ProcessDisclosureContext": { useProcessDetail: (id, defaultOpen = false) => [
      state.get(id) ?? defaultOpen, () => state.set(id, !(state.get(id) ?? defaultOpen)),
    ] },
    "./DagExecutionRow": { DagExecutionRow: "dag" },
    "./FileChangeBlock": { FileChangeBlock: ({ change }) => change.before + change.after },
    "./toolPresentation": { presentTool },
    "./format": { formatMs: value => `${value}ms` },
  };
  function load(name) {
    if (dependencies[name]) return dependencies[name];
    if (name !== "./ProcessToolRow" && name !== "./ProcessSectionRow") return require(name);
    if (cache.has(name)) return cache.get(name);
    const module = { exports: {} };
    const filename = path.resolve(__dirname, "../src/components/chat-timeline", name + ".tsx");
    vm.runInNewContext(ts.transpileModule(fs.readFileSync(filename, "utf8"), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
    }).outputText, { module, exports: module.exports, require: load });
    cache.set(name, module.exports);
    return module.exports;
  }
  function render(node) {
    if (Array.isArray(node)) return node.map(render);
    if (!node || typeof node !== "object") return node;
    if (typeof node.type === "function") return render(node.type(node.props));
    return { ...node, props: { ...node.props, children: render(node.props.children) } };
  }
  const { ProcessSectionRow } = load("./ProcessSectionRow");
  return () => render(ProcessSectionRow({ processing: false, section: {
    id: "execution-one", kind: "execution",
    blocks: tools.map(tool => ({ kind: "tool", id: tool.call_id, tool })),
  } }));
}
function nodes(tree) {
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  if (!tree || typeof tree !== "object") return [];
  return [tree, ...nodes(tree.props.children)];
}
function text(tree) {
  if (Array.isArray(tree)) return tree.map(text).join("");
  if (tree == null || typeof tree === "boolean") return "";
  if (typeof tree !== "object") return String(tree);
  return text(tree.props.children);
}

test("one click on a singleton skill, file, directory or command reveals its content", () => {
  for (const [fields, expected] of [
    [{ output: { name: "example-skill", body: "Skill instructions" } }, "Skill instructions"],
    [{ name: "read_file", output: { path: "src/a.ts", offset: 118, content: "   118\tconst a = 1;" } }, "const a = 1;"],
    [{ name: "list_dir", output: { path: "src", entries: [{ name: "nested", dir: true }, { name: "a.ts" }] } }, "nested/\na.ts"],
    [{ name: "bash", args: '{"command":"build"}', output: { stdout: "build passed", exit_code: 0 } }, "build passed"],
    [{ name: "edit_file", args: '{"old":"old value","new":"new value"}' }, "old valuenew value"],
  ]) {
    const draw = timelineFixture([tool(fields)]);
    let tree = draw();
    assert.equal(nodes(tree).filter(node => node.type === "button").length, 1);
    assert.equal(text(tree).includes(expected), false);
    nodes(tree).find(node => node.type === "button").props.onClick();
    tree = draw();
    assert.ok(text(tree).includes(expected));
    assert.doesNotMatch(text(tree), /查看详情|原始结果|退出码|操作失败|已加载/);
    nodes(tree).find(node => node.type === "button").props.onClick();
    assert.equal(text(draw()).includes(expected), false);
  }
});

test("file line ranges stay in the only clickable title and errors retain stderr", () => {
  const draw = timelineFixture([tool({ name: "read_file", output: { path: "a.ts", offset: 118, content: "   118\tfirst\n   119\tsecond" } })]);
  let tree = draw();
  assert.match(text(nodes(tree).find(node => node.type === "button")), /读取文件.*118–119/);
  nodes(tree).find(node => node.type === "button").props.onClick();
  tree = draw();
  assert.equal(text(tree).match(/118–119/g).length, 1);
  const failure = timelineFixture([tool({ name: "bash", status: "error", args: '{"command":"build"}', output: { stderr: "actual error", exit_code: 2 } })]);
  nodes(failure()).find(node => node.type === "button").props.onClick();
  assert.match(text(failure()), /失败.*actual error/);
  assert.doesNotMatch(text(failure()), /退出码|操作失败/);
});

test("grouped tools retain their container and each row opens its own details once", () => {
  const draw = timelineFixture([
    tool({ call_id: "skill", output: { name: "one", body: "skill body" } }),
    tool({ call_id: "dir", name: "list_dir", output: { entries: [{ name: "a.ts" }] } }),
  ]);
  nodes(draw()).find(node => node.type === "button").props.onClick();
  const skill = nodes(draw()).find(node => node.type === "button" && text(node).includes("使用技能"));
  skill.props.onClick();
  assert.match(text(draw()), /skill body/);
  const directory = nodes(draw()).find(node => node.type === "button" && text(node).includes("查看目录"));
  directory.props.onClick();
  assert.match(text(draw()), /a\.ts/);
  assert.doesNotMatch(text(draw()), /查看详情|原始结果/);
});
