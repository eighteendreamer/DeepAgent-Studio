const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const ts = require("typescript");
const locales = require("../src/locales/zh.json");

const source = readFileSync(join(__dirname, "../src/components/ApprovalDialog.tsx"), "utf8");
const moduleExports = {};
const code = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX,
  },
}).outputText;
vm.runInNewContext(code, {
  exports: moduleExports,
  module: { exports: moduleExports },
  require(name) {
    if (name === "react-i18next") return { useTranslation: () => ({
      t: (key, args = {}) => key.split(".").reduce((value, part) => value?.[part], locales)
        ?.replace(/\{\{(\w+)\}\}/g, (_, part) => args[part] ?? "") ?? key,
    }) };
    if (name === "./ui/Panel") return { Panel: "section" };
    if (name === "./shadcn/button") return { Button: "button" };
    if (name === "./shadcn/dropdown-menu") return {
      DropdownMenu: "menu-root", DropdownMenuTrigger: "menu-trigger",
      DropdownMenuContent: "menu", DropdownMenuItem: "menuitem",
    };
    if (name === "lucide-react") return new Proxy({}, { get: (_, key) => `icon-${key}` });
    return require(name);
  },
});

const request = {
  call_id: "call-edit", tool: "edit_file", risk: "medium",
  reason: "拆分登录模块：写入表单校验 validateLoginForm",
  arguments: '{"path":"src/auth/validators.ts"}',
};
function nodes(tree) {
  if (!tree || typeof tree !== "object") return [];
  return [tree, ...[tree.props?.children].flat(Infinity).flatMap(nodes)];
}
function text(tree) {
  if (typeof tree === "string" || typeof tree === "number") return String(tree);
  if (!tree || typeof tree !== "object") return "";
  return [tree?.props?.children].flat(Infinity).map(text).join("");
}
function fixture(overrides = {}) {
  const decisions = [];
  const tree = moduleExports.ApprovalDialog({
    request, onApprove: (...args) => decisions.push(["approve", ...args]),
    onReject: (...args) => decisions.push(["reject", ...args]), ...overrides,
  });
  return { tree, all: nodes(tree), decisions };
}

test("an absent request never creates an approval surface", () => {
  assert.equal(fixture({ request: null }).tree, null);
});

test("approval is a bordered nonmodal floating surface with one primary single-call action", () => {
  const f = fixture();
  assert.equal(f.tree.props.role, "region");
  assert.match(f.tree.props.className, /rounded-3xl/);
  assert.match(f.tree.props.className, /border-border-theme/);
  assert.ok(f.all.some(node => node.type === "pre" && text(node) === "src/auth/validators.ts"));
  const approve = f.all.find(node => node.type === "button" && text(node) === "允许一次");
  assert.ok(approve);
  assert.equal(approve.props.variant ?? "default", "default");
  approve.props.onClick();
  assert.deepEqual(f.decisions, [["approve", request]]);
  assert.ok(!f.all.some(node => node.type === "menu-trigger"));
});

test("only backend-advertised allowances appear, and each action keeps its exact scope", () => {
  const scoped = { ...request, tool: "bash", arguments: '{"command":"git status --short"}',
    scopes: ["single_call", "command_prefix", "tool_session", "unknown"], command_prefix: "git status" };
  const f = fixture({ request: scoped, queueCount: 3 });
  assert.match(text(f.tree), /还有 2 个待审批/);
  const items = f.all.filter(node => node.type === "menuitem");
  assert.equal(items.length, 2);
  assert.match(text(items[0]), /git status/);
  items.forEach(item => item.props.onSelect());
  f.all.find(node => node.type === "button" && text(node) === "拒绝").props.onClick();
  assert.deepEqual(f.decisions, [
    ["approve", scoped, "command_prefix"], ["approve", scoped, "tool_session"], ["reject", scoped],
  ]);
  assert.ok(f.all.some(node => node.type === "pre" && text(node) === "git status --short"));
});

test("a missing command prefix and unsupported scopes cannot expose a standing grant", () => {
  for (const scopes of [[], ["command_prefix"], ["unknown"]]) {
    const f = fixture({ request: { ...request, scopes } });
    assert.ok(!f.all.some(node => node.type === "menuitem" || node.type === "menu-trigger"));
  }
});

test("disabled approval disables every action including the scope menu", () => {
  const f = fixture({ disabled: true,
    request: { ...request, scopes: ["tool_session"] } });
  const actions = f.all.filter(node => node.type === "button" || node.type === "menuitem");
  assert.ok(actions.length >= 4);
  assert.ok(actions.every(node => node.props.disabled === true));
});

test("malformed and multiline arguments stay readable without HTML injection", () => {
  for (const args of ["<script>not executable</script>\nnext", '{"command":"echo one\\necho two"}']) {
    const f = fixture({ request: { ...request, arguments: args } });
    const content = f.all.find(node => node.type === "pre");
    assert.ok(content);
    assert.equal(content.props.dangerouslySetInnerHTML, undefined);
    assert.match(content.props.className, /whitespace-pre-wrap/);
    assert.match(text(content), /\n/);
  }
});

test("approval shares the question overlay and covers the composer instead of sitting above it", () => {
  const chat = readFileSync(join(__dirname, "../src/components/ChatView.tsx"), "utf8");
  const start = chat.indexOf("{(question || approval) && (");
  assert.notEqual(start, -1, "both decisions must use the same overlay host");
  const end = chat.indexOf("<SessionUsageBar", start);
  const anchor = chat.slice(start, end);
  assert.match(anchor, /absolute/);
  assert.match(anchor, /bottom-6/);
  assert.match(anchor, /z-40/);
  assert.match(anchor, /AskUserQuestionCard/);
  assert.match(anchor, /ApprovalDialog/);
  assert.match(anchor, /maxHeight:/);
  assert.doesNotMatch(chat, /--approval-available-height|calc\(100% \+ 12px\)/);
  assert.doesNotMatch(chat.slice(chat.indexOf("ref={composerFrameRef}")), /ApprovalDialog/);
});
