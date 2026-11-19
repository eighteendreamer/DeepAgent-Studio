const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");

test("working animation pauses offscreen, hidden, reduced and settled; cleanup stops every frame", () => {
  let frameId = 0, intersection;
  const frames = new Map(), handlers = new Map(), children = [];
  const reduced = { matches: false, addEventListener: (_, fn) => handlers.set("reduced", fn), removeEventListener: () => handlers.delete("reduced") };
  const doc = {
    hidden: false,
    createElementNS: () => ({ setAttribute() {}, remove() { children.splice(children.indexOf(this), 1); } }),
    addEventListener: (name, fn) => handlers.set(name, fn), removeEventListener: name => handlers.delete(name),
  };
  const module = { exports: {} };
  const source = fs.readFileSync(path.resolve(__dirname, "../src/components/chat-timeline/workingOrbAnimation.ts"), "utf8");
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText, {
    module, exports: module.exports, document: doc, window: { matchMedia: () => reduced },
    requestAnimationFrame: fn => { frames.set(++frameId, fn); return frameId; }, cancelAnimationFrame: id => frames.delete(id),
    IntersectionObserver: class { constructor(fn) { intersection = fn; } observe() {} disconnect() {} },
  });
  const mount = active => module.exports.mountWorkingOrb({ appendChild: child => children.push(child) }, active);
  const dispose = mount(true);
  assert.equal(children.length, 80);
  assert.equal(frames.size, 1);
  intersection([{ isIntersecting: false }]); assert.equal(frames.size, 0);
  intersection([{ isIntersecting: true }]); assert.equal(frames.size, 1);
  doc.hidden = true; handlers.get("visibilitychange")(); assert.equal(frames.size, 0);
  doc.hidden = false; handlers.get("visibilitychange")(); assert.equal(frames.size, 1);
  reduced.matches = true; handlers.get("reduced")(); assert.equal(frames.size, 0);
  reduced.matches = false; handlers.get("reduced")(); assert.equal(frames.size, 1);
  const [id, tick] = frames.entries().next().value; frames.delete(id); tick(100);
  assert.equal(frames.size, 1);
  dispose(); assert.equal(frames.size, 0); assert.equal(children.length, 0); assert.equal(handlers.size, 0);
  const settle = mount(false); assert.equal(frames.size, 0); settle();
  const strictRemount = mount(true); assert.equal(children.length, 80); strictRemount(); assert.equal(children.length, 0);
});
