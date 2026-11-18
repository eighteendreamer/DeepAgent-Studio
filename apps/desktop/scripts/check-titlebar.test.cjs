const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function host(native = true, label = 'main') {
  const slots = [], effects = [], calls = [], timers = new Map();
  let cursor = 0, resize, maximized = false, nextTimer = 0;
  const current = { label, isMaximized: async () => maximized,
    onResized: async callback => { resize = callback; return () => calls.push('unlisten'); },
    minimize: async () => calls.push('minimize'), close: async () => calls.push('close'),
    toggleMaximize: async () => { maximized = !maximized; calls.push('toggleMaximize'); },
    setFocus: async () => calls.push('focus'),
  };
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(path.resolve(__dirname, '../src/components/CustomTitleBar.tsx'), 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, console,
    window: native ? { __TAURI_INTERNALS__: {} } : {}, navigator: { userAgent: 'Windows' },
    setTimeout(fn, ms) { const id = ++nextTimer; timers.set(id, { fn, ms }); return id; },
    clearTimeout(id) { timers.delete(id); }, require(name) {
      if (name === 'react') return {
        useState(value) { const i = cursor++; if (!(i in slots)) slots[i] = value; return [slots[i], next => { slots[i] = next; }]; },
        useRef(value) { const i = cursor++; return slots[i] ??= { current: value }; },
        useEffect(fn, deps) { const i = cursor++; if (!slots[i] || deps.some((v, j) => v !== slots[i].deps[j])) {
          slots[i]?.cleanup?.(); slots[i] = { deps }; effects.push(() => { slots[i].cleanup = fn(); });
        } },
      };
      if (name === 'react/jsx-runtime') return { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }) };
      if (name === 'react-i18next') return { useTranslation: () => ({ t: value => value }) };
      if (name === '@tauri-apps/api/window') return { getCurrentWindow: () => current };
      if (name === '@tauri-apps/api/core') return { invoke: async command => calls.push(command) };
      if (name === './decorumTitlebar') return { NAV_ROW_ID: 'decorum-nav-row', isMainWindow: async () => !native || label === 'main' };
      if (name === './shadcn/utils') return { cn: (...values) => values.filter(Boolean).join(' ') };
      if (name === './message') return { message: { error: value => calls.push(`error:${value}`) } };
      return new Proxy({}, { get: (_, name) => String(name) });
    },
  });
  const render = () => {
    cursor = 0;
    const tree = module.exports.CustomTitleBar({ onToggleSidebar() {}, onBack() {}, onForward() {}, canGoBack: true, canGoForward: false });
    effects.splice(0).forEach(fn => fn());
    return tree;
  };
  const walk = (node, predicate) => Array.isArray(node) ? node.map(n => walk(n, predicate)).find(Boolean)
    : !node || typeof node !== 'object' ? undefined : predicate(node) ? node : walk(node.props?.children, predicate);
  return { calls, timers, render,
    find: predicate => walk(render(), predicate),
    button: label => walk(render(), node => node.props?.['aria-label'] === label),
    resize: async () => { await resize(); },
    dispose: () => slots.forEach(value => value?.cleanup?.()),
  };
}
const settle = () => new Promise(resolve => setImmediate(resolve));

test('navigation renders immediately without a decorum injection or native window', async () => {
  const h = host(false);
  assert.equal(h.find(node => node.props?.id === 'decorum-nav-row').type, 'div');
  await settle();
  assert.equal(h.button('titleBar.close'), undefined);
  assert.deepEqual(h.calls, []);
  h.dispose();
});

test('window controls call the existing native lifecycle and release their resize subscription', async () => {
  const h = host(); h.render(); await settle();
  h.button('titleBar.minimize').props.onClick();
  h.button('titleBar.maximize').props.onClick();
  await h.resize();
  await settle();
  assert.ok(h.button('titleBar.restore'));
  h.button('titleBar.close').props.onClick();
  h.dispose();
  assert.deepEqual(h.calls, ['minimize', 'toggleMaximize', 'close', 'unlisten']);
});

test('Windows snap hover is cancellable and uses the existing decorum command', async () => {
  const h = host(); h.render(); await settle();
  h.button('titleBar.maximize').props.onMouseEnter();
  assert.equal([...h.timers.values()][0].ms, 620);
  h.button('titleBar.maximize').props.onMouseLeave();
  assert.equal(h.timers.size, 0);
  h.button('titleBar.maximize').props.onMouseEnter();
  [...h.timers.values()][0].fn(); await settle();
  assert.deepEqual(h.calls, ['focus', 'plugin:decorum|show_snap_overlay']);
  h.dispose();
  assert.equal(h.timers.size, 0);
});

test('session windows retain their system titlebar', async () => {
  const h = host(true, 'session-one'); h.render(); await settle();
  assert.equal(h.render(), null);
  h.dispose();
});
