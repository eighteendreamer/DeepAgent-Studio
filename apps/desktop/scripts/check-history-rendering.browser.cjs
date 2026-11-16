// Real app rendering with synthetic history and delayed renderer modules.
// No native IPC, credentials, persisted user history or model requests.
const { chromium } = require("playwright");
const ts = require("typescript");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const assert = require("node:assert/strict");
const baseline = process.argv.includes("--baseline");
const root = path.resolve(__dirname, "../../..");
const output = path.resolve(__dirname, "../.tmp/history-rendering", baseline ? "before" : "after");
fs.mkdirSync(output, { recursive: true });

async function scenario(browser, math) {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = [];
  page.on("pageerror", e => errors.push(e.message));
  const text = math ? "公式 $x^2 + y^2$ 与化学式 $\\ce{H2O}$" : "## 历史验收\n\n| 日期 | 天气 |\n| --- | --- |\n| 周一 | 小雨 |\n\n**格式化内容**";
  const summary = { id: "legacy", title: "历史渲染验收", mode: "normal", created_at: 1, updated_at: 1, ended: false, pinned: false };
  const detail = { summary, timeline: [], stats: {}, conversation: [
    { role: "user", content: "验收内容", parts: [] },
    { role: "assistant", content: text, parts: [{ kind: "text", text }] },
  ] };
  await page.addInitScript(({ detail }) => {
    window.historyFixture = { detail, reads: 0, planReads: 0, rawFrames: 0 };
  }, { detail });
  const bodies = {
    getSettings: `{ return {configured:true,available_models:["deepseek-flash"],model_capabilities:[],chat_model:"deepseek-flash",reasoning_effort:"high"}; }`,
    listSessions: `{ return [window.historyFixture.detail.summary]; }`,
    getSessionDetail: `{ window.historyFixture.reads++; return window.historyFixture.detail; }`,
    getPlanMode: `{ window.historyFixture.planReads++; throw new Error("unknown legacy plan mode"); }`,
  };
  await page.route("**/src/api.ts", async route => {
    const response = await route.fetch();
    let source = await response.text();
    const ast = ts.createSourceFile("api.js", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    const edits = ast.statements.filter(n => ts.isFunctionDeclaration(n) && bodies[n.name?.text])
      .map(n => ({ start: n.body.getStart(ast), end: n.body.end, body: bodies[n.name.text] }));
    assert.equal(edits.length, Object.keys(bodies).length);
    for (const e of edits.sort((a,b) => b.start-a.start)) source = source.slice(0,e.start)+e.body+source.slice(e.end);
    await route.fulfill({ response, body: source });
  });
  if (baseline) {
    // Reproduce the prior raw-text fallback without reverting the working tree.
    for (const name of ["MarkdownText"]) {
      await page.route(new RegExp(`/src/components/${name}(?:\\.tsx)?(?:\\?.*)?$`), async route => {
        let source = execFileSync("git", ["show", `0a83f34b:apps/desktop/src/components/${name}.tsx`], { cwd: root, encoding: "utf8" });
        if (name === "MarkdownText") source += "\nexport async function preloadMarkdownRenderers() {}\n";
        const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020, jsx: ts.JsxEmit.ReactJSX } }).outputText;
        // Resolve bare imports through Vite's dependency entries in this fixture.
        const rewritten = code.replace(/import \{([^}]+)\} from "react";/g,
          (_, names) => `import React from "/node_modules/.vite/deps/react.js"; const {${names}} = React;`)
          .replace(/import \{([^}]+)\} from "react\/jsx-runtime";/g,
            (_, names) => `import JSX from "/node_modules/.vite/deps/react_jsx-runtime.js"; const {${names.replace(/ as /g, ": ")}} = JSX;`);
        await route.fulfill({ contentType: "application/javascript", body: rewritten });
      });
    }
  }
  let release;
  const held = new Promise(resolve => { release = resolve; });
  let delayedRequests = 0;
  await page.route(math ? /.*rehype-katex.*\.js.*/ : /.*\/BasicMarkdownText(?:\.tsx)?(?:\?.*)?$/, async route => {
    delayedRequests++;
    await held;
    await route.continue();
  });
  try {
    await page.goto(process.env.COMPOSER_TEST_URL || "http://localhost:1420/", { waitUntil: "domcontentloaded", timeout: 120000 });
    const row = page.getByText("历史渲染验收", { exact: true }).first();
    await row.waitFor();
    await page.waitForFunction(() => window.historyFixture.reads === 1);
    assert.equal(await page.evaluate(() => window.historyFixture.planReads), 0);
    await page.evaluate(() => {
      function frame() {
        const body = document.body.innerText;
        if (body.includes("| --- |") || body.includes("$x^2") || body.includes("\\ce{H2O}")) window.historyFixture.rawFrames++;
        window.historyFixture.frame = requestAnimationFrame(frame);
      }
      requestAnimationFrame(frame);
    });
    await row.click();
    await page.getByText("验收内容", { exact: true }).first().waitFor();
    await page.waitForTimeout(500);
    assert.ok(delayedRequests > 0, "actually hold the async renderer, not an already loaded page");
    const during = await page.locator("body").innerText();
    const rawShown = during.includes(math ? "$x^2" : "| --- |");
    assert.equal(rawShown, baseline, "source text must never be the loading placeholder");
    assert.doesNotMatch(during, /计划模式状态读取失败|计划模式状态尚未确认/);
    assert.equal(await page.evaluate(() => window.historyFixture.reads), 1, "navigation consumes startup prefetch");
    await page.screenshot({ path: path.join(output, `${math ? "math" : "table"}-loading.png`) });
    release();
    await page.locator(math ? ".katex" : ".markdown-body table").first().waitFor({ timeout: 30000 });
    await page.waitForTimeout(150);
    if (math) assert.equal(await page.locator(".katex-error").count(), 0);
    else assert.equal(await page.locator(".markdown-body table tbody tr").count(), 1);
    const rawFrames = await page.evaluate(() => { cancelAnimationFrame(window.historyFixture.frame); return window.historyFixture.rawFrames; });
    if (!baseline) assert.equal(rawFrames, 0);
    assert.deepEqual(errors, []);
    await page.screenshot({ path: path.join(output, `${math ? "math" : "table"}-rendered.png`) });
    return { math, baseline, rawShown, rawFrames, delayedRequests, historyReads: 1, pageErrors: errors };
  } catch (error) {
    console.error(JSON.stringify({ math, baseline, pageErrors: errors, body: (await page.locator("body").innerText()).slice(0, 900) }));
    throw error;
  } finally { release(); await page.close(); }
}
(async () => {
  const browser = await chromium.launch({ headless: true, channel: "chrome" });
  try {
    const results = [];
    // The old basic renderer alone proves the raw-source flash. The fixed
    // path additionally exercises math runtime + stylesheet readiness.
    for (const math of baseline ? [false] : [false, true]) results.push(await scenario(browser, math));
    fs.writeFileSync(path.join(output, "result.json"), JSON.stringify(results, null, 2));
    console.log(JSON.stringify(results));
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
