const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");

const root = path.join(__dirname, "../src/components/ui");
function load(file, environment, importer = require) {
  const module = { exports: {} };
  const code = ts.transpileModule(fs.readFileSync(path.join(root, file), "utf8"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  vm.runInNewContext(code, { module, exports: module.exports, require: importer, ...environment });
  return module.exports;
}

function fixture(width = 896) {
  const cells = [], pending = [], sets = [], timelines = [];
  const media = { matches: false, addEventListener() {}, removeEventListener() {} };
  const environment = { window: { matchMedia: () => media },
    getComputedStyle: node => ({ opacity: String(node.style.opacity ?? 0), backgroundColor: "rgb(243,244,246)" }),
    ResizeObserver: class { observe() {} disconnect() {} },
  };
  const motion = load("springMotion.ts", environment);
  const element = () => ({ style: {}, dataset: {}, inert: false });
  const textarea = { ...element(), value: "你是谁", scrollHeight: 24, focusCount: 0,
    focus() { this.focusCount++; }, setSelectionRange() {} };
  const editButton = { ...element(), focusCount: 0, focus() { this.focusCount++; } };
  const nodes = {
    slotRef: { ...element(), clientWidth: width }, shellRef: element(),
    compactRef: { ...element(), offsetWidth: 100, offsetHeight: 48 },
    editorRef: { ...element(), get offsetHeight() { return parseFloat(textarea.style.height || "48") + 70; }, querySelectorAll: () => [element(), element()] },
    textareaRef: textarea, actionsRef: element(), editButtonRef: editButton,
  };
  const gsap = { set(target, vars) { sets.push({ target, vars }); Object.assign(target.style, vars); },
    killTweensOf() {}, timeline(options = {}) {
      const steps = [];
      const timeline = { steps, killed: false,
        to(target, vars, at) { steps.push({ target, vars, at }); return this; },
        fromTo(target, from, vars, at) { steps.push({ target, from, vars, at }); return this; },
        call(callback, args, at) { steps.push({ callback, at }); return this; },
        kill() { this.killed = true; },
        complete() {
          assert.equal(this.killed, false);
          for (const step of steps) {
            if (step.callback) step.callback();
            else if (step.target.style) Object.assign(step.target.style, step.vars);
          }
          options.onComplete?.();
        },
      };
      timelines.push(timeline); return timeline;
    },
  };
  let cursor = 0;
  const react = { useRef(initial) { return cells[cursor++] ??= { current: initial }; },
    useLayoutEffect(effect, deps) {
      const cell = cells[cursor++] ??= {};
      if (!cell.deps || deps.some((value, index) => value !== cell.deps[index])) pending.push({ cell, effect, deps });
    },
  };
  const { useMessageEditMotion } = load("useMessageEditMotion.ts", environment, name => {
    if (name === "react") return react;
    if (name === "gsap") return { __esModule: true, default: gsap };
    if (name === "./springMotion") return motion;
    if (name === "../../theme/continuousCorners") return { continuousRadius: value => value };
    throw Error(`unexpected import: ${name}`);
  });
  const block = {};
  const render = (open, draft = "你是谁") => {
    cursor = 0;
    const refs = useMessageEditMotion(open, draft, block);
    for (const [key, node] of Object.entries(nodes)) refs[key].current = node;
    for (const { cell, effect, deps } of pending.splice(0)) {
      cell.cleanup?.(); cell.deps = deps; cell.cleanup = effect();
    }
  };
  render(false);
  return { nodes, sets, timelines, motion, media, render };
}

test("E width stays compact on desktop and fits narrow containers", () => {
  for (const [available, expected] of [[896, 582.4], [512, 332.8], [274, 274]]) {
    const f = fixture(available); f.render(true);
    assert.equal(parseFloat(f.nodes.editorRef.style.width), expected);
    assert.ok(expected <= available);
  }
});

test("shell and answer slot share A's height timing without scaling the text", () => {
  const f = fixture(); f.render(true);
  const steps = f.timelines.at(-1).steps;
  const shell = steps.find(step => step.target === f.nodes.shellRef && step.vars.height !== undefined);
  const slot = steps.find(step => step.target === f.nodes.slotRef);
  assert.equal(shell.vars.duration, f.motion.SHARE_MOTION.open.height.duration);
  assert.equal(shell.at, f.motion.SHARE_MOTION.open.height.delay);
  assert.equal(slot.vars.duration, shell.vars.duration);
  assert.equal(slot.at, shell.at);
  assert.ok(steps.every(step => !step.vars || !("scale" in step.vars || "scaleY" in step.vars)));
});

test("canceling an edited draft retains the full return morph instead of snapping closed", () => {
  const f = fixture(); f.render(true); f.timelines.at(-1).complete();
  f.render(true, "修改后的消息");
  const before = f.sets.length;
  f.render(false, "你是谁");
  assert.equal(f.nodes.shellRef.dataset.editAnimating, "true");
  assert.ok(!f.sets.slice(before).some(({ target, vars }) =>
    (target === f.nodes.shellRef || target === f.nodes.slotRef) && ("width" in vars || "height" in vars)));
  const close = f.timelines.at(-1);
  assert.equal(close.steps.find(step => step.target === f.nodes.shellRef && "width" in step.vars).vars.duration,
    f.motion.SHARE_MOTION.close.width.duration);
  close.complete();
  assert.equal(f.nodes.shellRef.dataset.editAnimating, "false");
  assert.equal(f.nodes.editButtonRef.focusCount, 1);
});

test("rapid reopen cancels the previous return and leaves a usable editor", () => {
  const f = fixture(); f.render(true); f.timelines.at(-1).complete();
  f.render(false); const closing = f.timelines.at(-1);
  f.render(true); assert.equal(closing.killed, true);
  f.timelines.at(-1).complete();
  assert.equal(f.nodes.editorRef.inert, false);
  assert.equal(f.nodes.actionsRef.inert, true);
  assert.equal(f.nodes.shellRef.dataset.editAnimating, "false");
});

test("long drafts resize within E's height and reduced motion skips animation", () => {
  const f = fixture(); f.media.matches = true;
  f.nodes.textareaRef.scrollHeight = 800; f.render(true);
  assert.equal(f.nodes.textareaRef.style.height, "144px");
  assert.ok(f.timelines.at(-1).steps.every(step => !step.vars || step.vars.duration === 0));
});

test("editing a tall history bubble also shrinks its slot so the editor stays reachable", () => {
  const f = fixture();
  f.nodes.compactRef.offsetHeight = 720;
  f.nodes.textareaRef.scrollHeight = 800;
  f.render(true);
  const slot = f.timelines.at(-1).steps.find(step => step.target === f.nodes.slotRef);
  assert.equal(slot.vars.height, 222);
  assert.equal(slot.vars.duration, f.motion.SHARE_MOTION.open.height.duration);
});
