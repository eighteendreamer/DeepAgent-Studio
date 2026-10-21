import assert from "node:assert/strict";
import { test } from "node:test";
import postcss from "postcss";
import tailwindcss from "tailwindcss";
import continuousCorners from "./continuous-corners.postcss.mjs";
import {
  CONTINUOUS_CORNERS, CORNER_CURVATURE, CORNER_RADIUS_SCALE,
  continuousCornerStyle, continuousRadius,
} from "../src/theme/continuousCorners.ts";

async function transform(css) {
  return (await postcss([continuousCorners()]).process(css, { from: undefined })).root;
}
function declaration(root, selector, property) {
  let value;
  root.walkRules(rule => {
    if (rule.selector === selector) rule.walkDecls(property, decl => { value = decl.value.trim(); });
  });
  return value;
}

test("the approved 24px / 60% policy calibrates the native diagonal inset", () => {
  assert.deepEqual(CONTINUOUS_CORNERS, { radius: 24, smoothing: 0.6 });
  const nativeInset = 24 * CORNER_RADIUS_SCALE * (1 - 2 ** (-1 / 2 ** CORNER_CURVATURE));
  assert.ok(Math.abs(nativeInset - 24 * (1 - Math.SQRT1_2)) < 1e-10);
});

test("authored surfaces, nested radii and per-corner overrides share one policy", async () => {
  const root = await transform(`@continuous-corners;
    .composer { border-radius:24px }
    .nested { border-radius:inherit }
    .edge { border-top-right-radius:4px; border-bottom-left-radius:0 }
    .custom { border-radius:var(--card-radius, 12px) 8px / 4px 0 }
    .theme { --card-radius:12px }
  `);
  assert.equal(declaration(root, ".composer", "border-radius"), "calc((24px) * var(--continuous-corner-scale))");
  assert.equal(declaration(root, ".nested", "corner-shape"), "inherit");
  assert.equal(declaration(root, ".edge", "border-bottom-left-radius"), "0");
  assert.equal(declaration(root, ".edge", "border-top-right-radius"), "calc((4px) * var(--continuous-corner-scale))");
  assert.match(declaration(root, ".custom", "border-radius"), /var\(--card-radius, 12px\)/);
  assert.equal(declaration(root, ".theme", "--card-radius"), "12px");
  assert.match(root.toString(), /@supports \(corner-shape: superellipse\(2\)\)/);
  const defaults = root.nodes.find(node => node.type === "rule" && node.selector === ":root");
  assert.equal(defaults.nodes.find(node => node.prop === "--continuous-corner-scale").value, "1");
});

test("circles, capsules and the jelly thumb keep their proportional contours", async () => {
  const root = await transform(`
    .circle {border-radius:50%}
    .pill {border-radius:9999px !important}
    .jelly {border-radius:var(--left,50%) var(--right,50%) 50% 50%}
    .square {border-radius:0}
  `);
  for (const selector of [".circle", ".pill", ".jelly"]) {
    assert.equal(declaration(root, selector, "corner-shape"), "round");
  }
  assert.equal(declaration(root, ".circle", "border-radius"), "50%");
  assert.equal(declaration(root, ".pill", "border-radius"), "9999px");
  assert.equal(declaration(root, ".square", "border-radius"), "0");
});

test("actual Tailwind named, arbitrary and directional utilities are converted", async () => {
  const result = await postcss([
    tailwindcss({ content: [{ raw: "rounded-lg rounded-full rounded-[24px] rounded-tr-sm rounded-none", extension: "html" }], corePlugins: { preflight: false } }),
    continuousCorners(),
  ]).process("@tailwind utilities; @continuous-corners;", { from: undefined });
  assert.equal(declaration(result.root, ".rounded-lg", "border-radius"), "calc((0.5rem) * var(--continuous-corner-scale))");
  assert.equal(declaration(result.root, ".rounded-full", "corner-shape"), "round");
  assert.equal(declaration(result.root, ".rounded-\\[24px\\]", "border-radius"), "calc((24px) * var(--continuous-corner-scale))");
  assert.match(declaration(result.root, ".rounded-tr-sm", "border-top-right-radius"), /--continuous-corner-scale/);
  assert.equal(declaration(result.root, ".rounded-none", "border-radius"), "0px");
});

test("inline/canvas radii use the CSS scale and keep circle handles round", () => {
  assert.ok(Math.abs(continuousRadius(24, true) - 38.4) < 1e-10);
  assert.deepEqual(continuousCornerStyle(8, true), { borderRadius: 12.8 });
  assert.deepEqual(continuousCornerStyle("50%", true), { borderRadius: "50%", cornerShape: "round" });
  assert.deepEqual(continuousCornerStyle(9999, true), { borderRadius: 9999, cornerShape: "round" });
});

test("older WebViews retain the existing radius rather than invalidating layout", () => {
  assert.equal(continuousRadius(24, false), 24);
  assert.deepEqual(continuousCornerStyle(8, false), { borderRadius: 8 });
});
