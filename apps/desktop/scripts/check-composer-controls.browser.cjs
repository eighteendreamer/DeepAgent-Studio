// Isolated browser fixture: real app/components, synthetic API boundaries only.
// Requires Playwright on NODE_PATH and an existing Vite dev server; no native IPC.
const { chromium } = require("playwright");
const ts = require("typescript");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const baseline = process.argv.includes("--baseline");
const output = path.resolve(__dirname, "../.tmp/composer-controls", baseline ? "before" : "after");
fs.mkdirSync(output, { recursive: true });

const bodies = {
  getSettings: `{ window.fixture.reads++; return {...window.fixture.settings}; }`,
  listSkills: `{ window.fixture.skills++; return []; }`,
  listSessions: `{ return [{id:"legacy",title:"Legacy fixture",mode:"normal",created_at:1,updated_at:1,ended:false,pinned:false}]; }`,
  getPlanMode: `{ window.fixture.planReads++; throw new Error("Plan mode state is unknown for this legacy session; explicitly select /plan or /execute before continuing"); }`,
  setReasoningEffort: `{
    const f = window.fixture; f.saves.push(effort);
    await new Promise(resolve => setTimeout(resolve, f.saves.length === 1 ? 350 : 30));
    if (f.failNext) { f.failNext = false; throw new Error("fixture save failed"); }
    f.settings = {...f.settings, reasoning_effort:effort};
    emitSettingsChanged({reason:"reasoning_effort"});
    return {...f.settings};
  }`,
  setChatModel: `{
    window.fixture.settings = {...window.fixture.settings, chat_model:modelId};
    emitSettingsChanged({reason:"chat_model"}); return {...window.fixture.settings};
  }`,
};

async function main() {
  const browser = await chromium.launch({ channel: "chrome", headless: true });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const errors = [];
    page.on("pageerror", e => errors.push(e.message));
    await page.addInitScript(() => {
      window.fixture = { reads: 0, skills: 0, planReads: 0, saves: [], failNext: false,
        settings: { configured: true, available_models: ["deepseek-flash", "deepseek-v4-pro"],
          model_capabilities: [], chat_model: "deepseek-flash", reasoning_effort: "high" } };
    });
    await page.route("**/src/api.ts", async route => {
      const response = await route.fetch();
      let source = await response.text();
      const ast = ts.createSourceFile("api.js", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
      const edits = [];
      for (const node of ast.statements) {
        if (ts.isFunctionDeclaration(node) && bodies[node.name?.text]) {
          edits.push({ start: node.body.getStart(ast), end: node.body.end, text: bodies[node.name.text] });
        }
      }
      assert.equal(edits.length, Object.keys(bodies).length);
      for (const e of edits.sort((a, b) => b.start - a.start)) source = source.slice(0, e.start) + e.text + source.slice(e.end);
      await route.fulfill({ response, body: source });
    });
    await page.goto(process.env.COMPOSER_TEST_URL || "http://localhost:1420/", { waitUntil: "networkidle", timeout: 120000 });
    const trigger = page.getByRole("button", { name: /deepseek-flash/ }).first();
    await trigger.waitFor();
    const initial = await page.evaluate(() => ({ ...window.fixture }));
    if (!baseline) assert.equal(initial.planReads, 0, "home must not read the remembered legacy mode");
    await trigger.evaluate(node => {
      const shell = node.closest("[data-motion-id]");
      window.frames = [];
      node.addEventListener("click", () => {
        const start = performance.now();
        function sample(now) {
          const r = shell.getBoundingClientRect();
          window.frames.push({ time: now - start, width: r.width, height: r.height });
          if (now - start < 1200) requestAnimationFrame(sample);
        }
        requestAnimationFrame(sample);
      }, { once: true });
    });
    await trigger.click();
    await page.waitForTimeout(1300);
    const frames = await page.evaluate(() => window.frames);
    const last = frames.at(-1);
    const unsettled = frames.filter(f => Math.abs(f.width-last.width)>0.5 || Math.abs(f.height-last.height)>0.5);
    const stops = page.locator(".reasoning-effort-stops button");
    const layout = await stops.evaluateAll(nodes => nodes.map(n => ({ text: n.textContent, top: n.getBoundingClientRect().top })));
    await page.screenshot({ path: path.join(output, "model-menu.png") });
    if (!baseline) {
      assert.deepEqual(layout.map(n => n.text), ["无", "低", "中", "高"]);
      assert.ok(layout.every(n => Math.abs(n.top-layout[0].top)<1), "all four stops must occupy one row");
    }
    // Two real pointer selections before the first backend save completes.
    await stops.nth(1).click();
    await stops.nth(3).click();
    await page.waitForTimeout(900);
    const state = await page.evaluate(() => ({ ...window.fixture }));
    const pressed = await page.locator('.reasoning-effort-stops button[aria-pressed="true"]').innerText();
    if (!baseline) {
      assert.equal(state.settings.reasoning_effort, "max", "last user selection must win in storage");
      assert.equal(pressed, "高", "late responses must not rewind the slider");
      assert.equal(state.skills, initial.skills, "model/effort changes must not rescan skills");
    }
    const result = { baseline, homePlanReads: initial.planReads, layout,
      openSettledMs: unsettled.at(-1)?.time ?? 0,
      maxFrameGapMs: Math.max(...frames.slice(1).map((f,i) => f.time-frames[i].time)),
      selection: pressed, storedEffort: state.settings.reasoning_effort,
      saves: state.saves, extraSettingsReads: state.reads-initial.reads, extraSkillReads: state.skills-initial.skills,
      pageErrors: errors };
    if (!baseline) {
      const slider = page.getByRole("slider", { name: "推理强度" });
      await slider.focus();
      await page.keyboard.press("Home");
      await page.waitForFunction(() => window.fixture.settings.reasoning_effort === "none");
      await page.keyboard.press("End");
      await page.waitForFunction(() => window.fixture.settings.reasoning_effort === "max");
      const track = await page.locator(".reasoning-effort-input").boundingBox();
      await page.mouse.move(track.x + track.width - 16, track.y + track.height / 2);
      await page.mouse.down();
      await page.mouse.move(track.x + 16, track.y + track.height / 2, { steps: 12 });
      await page.mouse.up();
      await page.waitForFunction(() => window.fixture.settings.reasoning_effort === "none");
      await stops.nth(3).click();
      await page.waitForFunction(() => window.fixture.settings.reasoning_effort === "max");
      // Failed persistence restores the last saved selection and reports it.
      await page.evaluate(() => { window.fixture.failNext = true; });
      await stops.nth(1).click();
      await page.waitForFunction(() => !window.fixture.failNext);
      await page.waitForTimeout(100);
      assert.equal(await page.locator('.reasoning-effort-stops button[aria-pressed="true"]').innerText(), "高");
      assert.match(await page.locator("body").innerText(), /切换.*失败/);
      await page.locator('[data-menu-item="deepseek-v4-pro"]').click();
      await page.waitForFunction(() => window.fixture.settings.chat_model === "deepseek-v4-pro");
      await page.keyboard.press("Escape");
      await page.locator('[data-morph-panel]').waitFor({ state: "detached" });
      const updatedTrigger = page.getByRole("button", { name: /deepseek-v4-pro/ }).first();
      assert.match(await updatedTrigger.innerText(), /高/);
      await updatedTrigger.click();
      await page.waitForTimeout(80);
      await page.keyboard.press("Escape");
      await page.locator('[data-morph-panel]').waitFor({ state: "detached" });
      await page.emulateMedia({ reducedMotion: "reduce" });
      await updatedTrigger.click();
      await slider.waitFor();
      assert.equal(await page.locator('[data-morph-panel]').count(), 1);
      // Match the supplied screenshot's enlarged controls without label wrap.
      await page.setViewportSize({ width: 1920, height: 1200 });
      await page.evaluate(() => { document.body.style.zoom = "1.5"; });
      await page.waitForTimeout(150);
      const tops = await stops.evaluateAll(nodes => nodes.map(n => n.getBoundingClientRect().top));
      assert.ok(tops.every(top => Math.abs(top-tops[0])<1));
      await page.screenshot({ path: path.join(output, "model-menu-150-percent.png") });
      result.keyboardRollbackModelAndReducedMotionPassed = true;
    }
    assert.deepEqual(errors, []);
    fs.writeFileSync(path.join(output, "result.json"), JSON.stringify(result, null, 2));
    console.log(JSON.stringify(result));
  } finally { await browser.close(); }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
