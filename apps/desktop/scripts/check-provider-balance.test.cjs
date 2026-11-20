const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const ts = require("typescript");
const root = path.resolve(__dirname, "../src");

function transpile(source) {
  return ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText;
}

function balanceApi({ selected, balance, failure } = {}) {
  const source = fs.readFileSync(path.join(root, "api.ts"), "utf8");
  const ast = ts.createSourceFile("api.ts", source, ts.ScriptTarget.Latest, true);
  const fn = ast.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === "getBalance");
  const calls = [];
  const invoke = async (command, args) => {
    calls.push([command, args]);
    if (command === "harness_model_selection") return selected ?? null;
    if (command === "harness_balance") {
      if (failure) throw failure;
      return balance;
    }
    if (command === "get_balance") return { is_available: true, infos: [{ currency: "CNY", total_balance: "999" }] };
    throw new Error(`unexpected command ${command}`);
  };
  const module = { exports: {} };
  const harness = {
    getHarnessModelSelection: () => invoke("harness_model_selection"),
    fetchHarnessBalance: providerId => invoke("harness_balance", { providerId }),
    fetchSelectedHarnessBalance: async () => {
      const model = await invoke("harness_model_selection");
      return model ? invoke("harness_balance", { providerId: model.providerId }) : null;
    },
  };
  vm.runInNewContext(transpile(fn.getText(ast)), {
    exports: module.exports, module, getInvoke: () => invoke,
    require: name => { assert.equal(name, "./harnessModelsApi"); return harness; },
  });
  return { read: module.exports.getBalance, calls };
}

test("selected provider failures surface without querying another provider", async () => {
  const host = balanceApi({ selected: { providerId: "responses" }, failure: new Error("provider unavailable") });
  await assert.rejects(host.read(), /provider unavailable/);
  assert.equal(host.calls.filter(([command]) => command === "get_balance").length, 0);
});

test("unknown balance and availability remain unknown", async () => {
  const host = balanceApi({ selected: { providerId: "relay" }, balance: {
    available: null, amount: { amount: null, unit: "USD", unlimited: false },
  } });
  const result = await host.read();
  assert.equal(result.is_available, null);
  assert.equal(result.infos[0].total_balance, "");
  assert.equal(result.infos[0].currency, "USD");
});

test("unlimited remains separate from the numeric amount", async () => {
  const host = balanceApi({ selected: { providerId: "relay" }, balance: {
    available: true, amount: { amount: null, unit: "tokens", unlimited: true },
  } });
  const result = await host.read();
  assert.equal(result.infos[0].unlimited, true);
  assert.equal(result.infos[0].total_balance, "");
});

test("no model selection leaves balance unknown without a legacy request", async () => {
  const host = balanceApi();
  const result = await host.read();
  assert.equal(result.infos.length, 0);
  assert.equal(result.is_available, null);
  assert.deepEqual(host.calls.map(([command]) => command), ["harness_model_selection"]);
});

test("a provider with balance disabled does not fabricate a displayed amount", async () => {
  const host = balanceApi({ selected: { providerId: "relay" }, balance: {
    available: null, amount: { amount: null, unit: "none", unlimited: false },
  } });
  assert.equal((await host.read()).infos.length, 0);
});

test("both balance surfaces use the shared query without swallowing provider failures", () => {
  for (const component of ["BalanceChip", "SessionUsageBar"]) {
    const source = fs.readFileSync(path.join(root, `components/${component}.tsx`), "utf8");
    assert.doesNotMatch(source, /fetchSelectedHarnessBalance/);
    assert.match(source, /await getBalance\(\)/);
  }
});

test("balance formatting respects currency, tokens, unlimited and unknown amounts", () => {
  const module = { exports: {} };
  vm.runInNewContext(transpile(fs.readFileSync(path.join(root, "components/chat-timeline/format.ts"), "utf8")), {
    module, exports: module.exports,
  });
  const format = module.exports.formatBalanceAmount;
  assert.equal(typeof format, "function");
  assert.equal(format("USD", "12.5"), "$12.50");
  assert.equal(format("CNY", "0"), "¥0.00");
  assert.equal(format("tokens", "1234"), "1234 tokens");
  assert.equal(format("USD", "", true), "$∞");
  assert.equal(format("USD", ""), "—");
  assert.equal(format("", "7.5"), "7.50");
});

function componentHost(name) {
  const slots = [], effects = [], listeners = new Map(), reads = [];
  let cursor = 0;
  const hooks = {
    useState(value) {
      const i = cursor++;
      if (!(i in slots)) slots[i] = value;
      return [slots[i], next => { slots[i] = typeof next === "function" ? next(slots[i]) : next; }];
    },
    useRef(value) { const i = cursor++; return slots[i] ??= { current: value }; },
    useCallback(fn) { cursor++; return fn; },
    useEffect(fn) {
      const i = cursor++;
      if (!(i in slots)) { slots[i] = {}; effects.push(() => { slots[i].cleanup = fn(); }); }
    },
  };
  const module = { exports: {} };
  const source = fs.readFileSync(path.join(root, `components/${name}.tsx`), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: {
    module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX,
  } }).outputText;
  vm.runInNewContext(code, {
    module, exports: module.exports, console: { error() {} },
    window: {
      addEventListener(type, fn) { if (!listeners.has(type)) listeners.set(type, new Set()); listeners.get(type).add(fn); },
      removeEventListener(type, fn) { listeners.get(type)?.delete(fn); },
    },
    require(importName) {
      if (importName === "react") return hooks;
      if (importName === "react/jsx-runtime") return { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }), Fragment: "fragment" };
      if (importName === "react-i18next") return { useTranslation: () => ({ t: key => key }) };
      if (importName === "../api") return {
        SETTINGS_CHANGED_EVENT: "settings", unwrapRuntimeEvent: value => value,
        getBalance() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); reads.push({ resolve, reject }); return promise; },
      };
      if (importName === "./chat-timeline/format") return { formatTokens: String, formatCny: String, formatBalanceAmount: (unit, value) => `${unit}:${value}` };
      if (importName === "lucide-react" || importName === "./shadcn/hover-card") return {};
      throw new Error(`unexpected import: ${importName}`);
    },
  });
  return {
    reads,
    render() { cursor = 0; module.exports[name]({ sessionId: "session" }); effects.splice(0).forEach(fn => fn()); },
    changed() { listeners.get("settings")?.forEach(fn => fn({ detail: { affectsBalance: true } })); },
    balance() { return slots.find(slot => slot && typeof slot === "object" && "infos" in slot) ?? null; },
    unmount() { slots.forEach(slot => slot?.cleanup?.()); },
  };
}

for (const name of ["BalanceChip", "SessionUsageBar"]) {
  test(`${name} ignores an older provider reply arriving after the new one`, async () => {
    const host = componentHost(name);
    host.render();
    host.changed();
    assert.equal(host.reads.length, 2);
    const latest = { is_available: true, infos: [{ currency: "USD", total_balance: "2" }] };
    host.reads[1].resolve(latest);
    await new Promise(resolve => setImmediate(resolve));
    host.reads[0].resolve({ is_available: true, infos: [{ currency: "CNY", total_balance: "100" }] });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(host.balance(), latest);
    host.unmount();
  });
}
