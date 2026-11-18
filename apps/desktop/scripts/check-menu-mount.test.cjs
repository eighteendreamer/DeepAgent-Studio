const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function fixture() {
  const cells = [], effects = [], sets = [], timelines = [];
  let cursor = 0, reads = 0;
  const element = () => ({ style: {}, inert: false, dataset: {},
    contains: () => false, querySelector: () => element(), querySelectorAll: () => [],
    get offsetWidth() { reads++; return 120; }, get offsetHeight() { reads++; return 32; },
    get clientWidth() { reads++; return 500; },
    getBoundingClientRect() { reads++; return { left: 20, right: 140, top: 400, bottom: 432 }; } });
  const nodes = Array.from({ length: 6 }, element);
  const react = { useRef(initial) { return cells[cursor++] ??= { current: initial }; },
    useState(initial) { const cell = cells[cursor++] ??= { value: initial }; return [cell.value, value => { cell.value = value; }]; },
    useEffect() {}, useLayoutEffect(effect, deps) {
      const cell = cells[cursor++] ??= {};
      if (!cell.deps || deps.some((value, i) => value !== cell.deps[i])) effects.push({ cell, effect, deps });
    } };
  const gsap = { set(target, vars) { sets.push(vars); Object.assign(target.style, vars); }, killTweensOf() {},
    timeline(options = {}) { const timeline = { to() { return this; }, fromTo() { return this; },
      call() { return this; }, kill() {}, complete() { options.onComplete?.(); } };
      timelines.push(timeline); return timeline; }, to() { return { kill() {} }; } };
  const module = { exports: {} };
  const source = fs.readFileSync(path.join(__dirname, '../src/components/ui/MorphingMenuShell.tsx'), 'utf8');
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX,
  } }).outputText, { module, exports: module.exports,
    getComputedStyle() { reads++; return { borderTopLeftRadius: '16px', backgroundColor: 'rgb(255,255,255)' }; },
    document: { activeElement: null, addEventListener() {}, removeEventListener() {} },
    window: { innerWidth: 1280, innerHeight: 800, addEventListener() {}, removeEventListener() {},
      matchMedia: () => ({ addEventListener() {}, removeEventListener() {} }) },
    ResizeObserver: class { observe() {} disconnect() {} },
    require(name) {
      if (name === 'react') return react;
      if (name === 'react/jsx-runtime') return { jsx: (type, props) => ({ type, props }), jsxs: (type, props) => ({ type, props }) };
      if (name === 'gsap') return { __esModule: true, default: gsap };
      if (name === '../../theme/continuousCorners') return { continuousRadius: x => x, computedCornerCurvature: () => 2, CORNER_CURVATURE: 2 };
      if (name === '../shadcn/utils') return { cn: (...args) => args.join(' ') };
      if (name === './MorphPanelLayoutContext') return { MenuTriggerOpenContext: { Provider: 'provider' }, MorphPanelLayoutContext: { Provider: 'provider' } };
      if (name === './springMotion') return { prefersReducedMotion: () => false, motionColor: x => x,
        springEase: () => p => p, SHARE_MOTION: { open: { width: {}, height: {} }, close: { width: {}, height: {} } } };
      if (name.endsWith('.css')) return {};
      throw Error(name);
    } });
  const render = open => {
    cursor = 0;
    module.exports.MorphingMenuShell({ open, onOpenChange() {}, layoutId: 'test', trigger: 'trigger', children: 'panel' });
    nodes.forEach((node, i) => { cells[i].current = node; });
    for (const { cell, effect, deps } of effects.splice(0)) {
      cell.cleanup?.(); cell.deps = deps; cell.cleanup = effect();
    }
  };
  return { render, nodes, sets, timelines, get reads() { return reads; } };
}

test('mounting a closed menu performs no geometry reads or GSAP writes', () => {
  const f = fixture(); f.render(false);
  assert.equal(f.reads, 0);
  assert.equal(f.sets.length, 0);
});

test('opening and closing still measure and restore the compact control', () => {
  const f = fixture(); f.render(false); f.render(true);
  assert.ok(f.reads > 0);
  assert.equal(f.nodes[3].inert, true);
  assert.equal(f.nodes[4].inert, false);
  f.timelines.at(-1).complete();
  f.render(false); f.timelines.at(-1).complete();
  assert.equal(f.nodes[3].inert, false);
  assert.equal(f.nodes[4].inert, true);
  const before = f.reads, writes = f.sets.length;
  f.render(false);
  assert.equal(f.reads, before);
  assert.equal(f.sets.length, writes);
});
