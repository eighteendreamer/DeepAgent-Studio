const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const ts = require("typescript");

const source = fs.readFileSync(path.join(__dirname, "../src/components/onboardingEntrance.ts"), "utf8");
const moduleObject = { exports: {} };
vm.runInNewContext(ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
}).outputText, { module: moduleObject, exports: moduleObject.exports });
const { startOnboardingEntrance } = moduleObject.exports;

function fixture() {
  const tracks = [];
  const element = (width = 480, height = 36) => ({
    style: { visibility: "" },
    getBoundingClientRect: () => ({ width, height }),
    animate(keyframes, options) {
      let finish, reject, settled = false;
      const finished = new Promise((resolve, fail) => { finish = resolve; reject = fail; });
      const track = {
        element: this, keyframes, options, finished, cancelled: false,
        finish() { if (!settled) { settled = true; finish(); } },
        cancel() {
          track.cancelled = true;
          if (!settled) { settled = true; reject(Object.assign(new Error("cancelled"), { name: "AbortError" })); }
        },
      };
      tracks.push(track);
      return track;
    },
  });
  return { tracks, elements: {
    overlay: element(1200, 800), card: element(), welcome: element(480, 36),
    point: element(1, 1), doors: [element(), element()], seams: [element(), element()],
  } };
}

test("completion waits for both doors, even after every other animation has finished", async () => {
  const { elements, tracks } = fixture();
  const entrance = startOnboardingEntrance(elements, false);
  let complete = false;
  entrance.finished.then(value => { complete = value; });
  const lastDoor = tracks.find(track => track.element === elements.doors[1]);
  tracks.filter(track => track !== lastDoor).forEach(track => track.finish());
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(complete, false);
  lastDoor.finish();
  assert.equal(await entrance.finished, true);
  assert.equal(lastDoor.options.delay, 4600);
  assert.equal(lastDoor.options.duration, 1600);
});

test("text stays opaque while shrinking to exactly 1px; doors move with fixed width", async () => {
  const { elements, tracks } = fixture();
  const entrance = startOnboardingEntrance(elements, false);
  const text = tracks.find(track => track.element === elements.welcome);
  const last = text.keyframes[text.keyframes.length - 1];
  const scales = last.transform.match(/scale\(([^,]+), ([^)]+)\)/).slice(1).map(Number);
  assert.equal(scales[0] * 480, 1);
  assert.equal(scales[1] * 36, 1);
  assert.equal(last.opacity, 1);
  assert.equal(elements.welcome.style.visibility, "visible");
  for (const [index, door] of elements.doors.entries()) {
    const motion = tracks.find(track => track.element === door);
    assert.equal(motion.keyframes[1].transform, `translateX(${index ? "100%" : "-100%"})`);
    assert.equal(motion.keyframes.some(frame => "width" in frame), false);
  }
  text.finish();
  await Promise.resolve();
  assert.equal(elements.welcome.style.visibility, "hidden");
  entrance.cancel();
  assert.equal(await entrance.finished, false);
});

test("unmount/cancellation stops every track and cannot report a late completion", async () => {
  const { elements, tracks } = fixture();
  const entrance = startOnboardingEntrance(elements, false);
  entrance.cancel();
  tracks.forEach(track => track.finish());
  assert.equal(await entrance.finished, false);
  assert.equal(tracks.every(track => track.cancelled), true);
  assert.equal(elements.welcome.style.visibility, "");
});

test("reduced motion skips the cinematic entrance and still respects cancellation", async () => {
  const { elements, tracks } = fixture();
  assert.equal(await startOnboardingEntrance(elements, true).finished, true);
  assert.equal(tracks.length, 0);
  const cancelled = startOnboardingEntrance(elements, true);
  cancelled.cancel();
  assert.equal(await cancelled.finished, false);
});

test("animation setup failure releases already-created tracks and preserves the error", () => {
  const { elements, tracks } = fixture();
  const failure = new Error("animation setup failed");
  elements.doors[1].animate = () => { throw failure; };
  assert.throws(() => startOnboardingEntrance(elements, false), error => error === failure);
  assert.equal(tracks.every(track => track.cancelled), true);
  assert.equal(elements.welcome.style.visibility, "");
});
