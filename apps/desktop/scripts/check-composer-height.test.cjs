const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");

const root = path.resolve(__dirname, "..");
const composerSource = fs.readFileSync(path.join(root, "src/components/Composer.tsx"), "utf8");
const surfaceSource = fs.readFileSync(
  path.join(root, "src/components/ui/InputSurface.tsx"),
  "utf8",
);

/** Pull the `resizeTextarea` callback body out of Composer so the height
 *  contract can be exercised without mounting React or a real layout engine. */
function resizeTextarea() {
  const ast = ts.createSourceFile(
    "Composer.tsx",
    composerSource,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  );
  let body;
  function visit(node) {
    if (
      ts.isVariableDeclaration(node) &&
      node.name.getText(ast) === "resizeTextarea" &&
      node.initializer
    ) {
      body = node.initializer.arguments?.[0]?.getText(ast);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(body, "resizeTextarea must remain a useCallback in Composer");
  return (textarea, mirror, maxHeight = 300) => {
    const environment = {
      COMPOSER_TEXTAREA_MIN_HEIGHT: 60,
      textareaMaxHeight: maxHeight,
      textareaRef: { current: textarea },
      mirrorRef: { current: mirror },
    };
    // `body` is a function expression, so calling it runs the measurement.
    return vm.runInNewContext(`(${body})`, environment)();
  };
}

const element = (overrides = {}) => ({
  value: "",
  clientWidth: 800,
  scrollHeight: 60,
  style: {},
  ...overrides,
});

test("an empty draft is the minimum height without being measured", () => {
  const resize = resizeTextarea();
  // 240 is what a stale read would have produced; it must be ignored entirely.
  const textarea = element({ value: "", scrollHeight: 240 });
  const mirror = element({ scrollHeight: 240 });
  resize(textarea, mirror);
  assert.equal(textarea.style.height, "60px");
  assert.equal(textarea.style.overflowY, "hidden");
});

test("the mirror is what gets measured, not the textarea's own scrollHeight", () => {
  const resize = resizeTextarea();
  // The textarea reports the height currently written to it (the feedback loop
  // that froze an oversized box); the mirror reports the real content height.
  const textarea = element({ value: "one line", scrollHeight: 288 });
  const mirror = element({ scrollHeight: 72 });
  resize(textarea, mirror);
  assert.equal(textarea.style.height, "72px");
  assert.equal(textarea.style.overflowY, "hidden");
});

test("content beyond the max height clamps and scrolls", () => {
  const resize = resizeTextarea();
  const textarea = element({ value: "many lines" });
  const mirror = element({ scrollHeight: 512 });
  resize(textarea, mirror, 300);
  assert.equal(textarea.style.height, "300px");
  assert.equal(textarea.style.overflowY, "auto");
});

test("a missing mirror falls back to the textarea instead of collapsing to zero", () => {
  const resize = resizeTextarea();
  const textarea = element({ value: "one line", scrollHeight: 84 });
  resize(textarea, null);
  assert.equal(textarea.style.height, "84px");
});

test("a width change alone recomputes the height", () => {
  // The observer must key off width only; observing the height being written
  // would loop. Assert the guard exists in the same effect as the observer.
  const ast = ts.createSourceFile(
    "Composer.tsx",
    composerSource,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  );
  let observerEffect;
  function visit(node) {
    const text = node.getText?.(ast) ?? "";
    if (ts.isCallExpression(node) && node.expression.getText(ast) === "useLayoutEffect") {
      const body = node.arguments[0]?.getText(ast) ?? "";
      if (body.includes("new ResizeObserver") && body.includes("resizeTextarea()")) {
        observerEffect = body;
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
  assert.ok(observerEffect, "a ResizeObserver effect must call resizeTextarea");
  assert.match(observerEffect, /clientWidth/);
  assert.match(observerEffect, /requestAnimationFrame/);
  assert.doesNotMatch(
    observerEffect,
    /scrollHeight/,
    "the observer must not measure, or it re-enters on its own write",
  );
});

test("the composer surface does not animate height", () => {
  // transition-all included height, so one stale measurement became a slow
  // visible climb instead of an immediate correction.
  assert.doesNotMatch(surfaceSource, /transition-all/);
  assert.doesNotMatch(surfaceSource, /MOTION\.standard/);
  assert.match(surfaceSource, /transition-\[box-shadow/);
});
