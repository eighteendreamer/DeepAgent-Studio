const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const ts = require('typescript');

function fixture() {
  const listeners = new Map(), sets = [], tweens = [];
  let effect, reduced = false;
  const node = { matches: () => false,
    addEventListener(name, fn) { listeners.set(name, fn); },
    removeEventListener(name) { listeners.delete(name); } };
  const react = { useRef: () => ({ current: node }), useCallback: fn => fn,
    useEffect(fn) { effect = fn; } };
  const gsap = { set(...args) { sets.push(args); }, to(target, vars) {
    const tween = { target, vars, killed: false, kill() { this.killed = true; } };
    tweens.push(tween); return tween;
  } };
  const source = fs.readFileSync(path.join(__dirname, '../src/components/ui/useSpringPress.ts'), 'utf8');
  const module = { exports: {} };
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS,
  } }).outputText, { module, exports: module.exports,
    window: { addEventListener(name, fn) { listeners.set(name, fn); }, removeEventListener(name) { listeners.delete(name); } },
    require(name) {
      if (name === 'react') return react;
      if (name === 'gsap') return { __esModule: true, default: gsap };
      if (name === './springMotion') return { prefersReducedMotion: () => reduced, springEase: () => p => p };
      throw Error(name);
    } });
  module.exports.useSpringPress();
  const cleanup = effect();
  return { cleanup, sets, tweens, listeners, reduce() { reduced = true; } };
}

test('unmounting an untouched button never starts GSAP style cleanup', () => {
  const f = fixture(); f.cleanup();
  assert.equal(f.sets.length, 0);
  assert.equal(f.listeners.size, 0);
});

test('an actual pointer press still animates and cancels cleanly on unmount', () => {
  const f = fixture(); f.listeners.get('pointerdown')(); f.cleanup();
  assert.equal(f.tweens[0].vars.scale, 0.97);
  assert.equal(f.tweens[0].killed, true);
  assert.equal(f.sets.length, 1);
  assert.equal(f.sets[0][1].clearProps, 'scale');
});

test('release retains the return spring and cancels its tween', () => {
  const f = fixture(); f.listeners.get('pointerdown')(); f.listeners.get('pointerup')(); f.cleanup();
  assert.equal(f.tweens[0].killed, true);
  assert.equal(f.tweens[1].vars.scale, 1);
  assert.equal(f.tweens[1].killed, true);
});

test('reduced motion never initializes or cleans a transform', () => {
  const f = fixture(); f.reduce(); f.listeners.get('pointerdown')(); f.cleanup();
  assert.equal(f.tweens.length, 0);
  assert.equal(f.sets.length, 0);
});
