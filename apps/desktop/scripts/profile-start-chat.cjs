// Real Tauri WebView2 via CDP. Run only against an explicitly isolated test
// instance: each sample creates a local /help conversation, without inference.
// Playwright may be supplied by the workspace runtime through NODE_PATH.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const output = path.resolve(process.env.DESKTOP_AUDIT_OUTPUT || path.join(__dirname, '../../../target/start-chat-profile'));
const rate = Number(process.env.DESKTOP_AUDIT_CPU_RATE || 1);
const count = Number(process.env.DESKTOP_AUDIT_SAMPLES || 7);
const traceEnabled = process.env.DESKTOP_AUDIT_TRACE === '1';
assert.ok(Number.isFinite(rate) && rate >= 1 && rate <= 20);
assert.ok(Number.isInteger(count) && count >= 1 && count <= 50);
fs.mkdirSync(output, { recursive: true });

async function endTrace(cdp) {
  const completed = new Promise(resolve => cdp.once('Tracing.tracingComplete', resolve));
  await cdp.send('Tracing.end');
  const { stream } = await completed;
  let data = '';
  try {
    for (;;) {
      const chunk = await cdp.send('IO.read', { handle: stream });
      data += chunk.data;
      if (chunk.eof) return JSON.parse(data);
    }
  } finally { await cdp.send('IO.close', { handle: stream }); }
}

function traceSummary(trace) {
  const events = trace.traceEvents;
  const start = events.find(event => event.name === 'start-chat:send');
  const end = events.find(event => event.name === 'start-chat:dom');
  assert.ok(start && end, 'trace must contain the exact send and DOM boundaries');
  const duration = name => events.filter(event => event.ph === 'X' && event.name === name && event.tid === start.tid)
    .reduce((sum, event) => sum + Math.max(0, Math.min(end.ts, event.ts + (event.dur || 0)) - Math.max(start.ts, event.ts)) / 1000, 0);
  return { elapsedMs: (end.ts - start.ts) / 1000,
    styleRecalcMs: duration('UpdateLayoutTree'), layoutMs: duration('Layout'),
    layouts: events.filter(event => event.name === 'Layout' && event.ph === 'X' && event.tid === start.tid && event.ts >= start.ts && event.ts < end.ts).length };
}

async function main() {
  const browser = await chromium.connectOverCDP(process.env.DESKTOP_AUDIT_CDP || 'http://127.0.0.1:9223');
  let page;
  const deadline = Date.now() + 120000;
  do {
    page = browser.contexts().flatMap(context => context.pages()).find(page => page.url().startsWith('http://tauri.localhost/') && !page.url().includes('splash'));
    if (!page) await new Promise(resolve => setTimeout(resolve, 100));
  } while (!page && Date.now() < deadline);
  if (!page) await browser.close();
  assert.ok(page, 'an isolated native main WebView must exist');
  const cdp = await page.context().newCDPSession(page);
  const samples = [], errors = [];
  let tracing = false;
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.reload({ waitUntil: 'load' });
    await page.locator('.app-navigation-home').waitFor({ timeout: 120000 });
    assert.equal(await page.evaluate(() => !!window.__TAURI_INTERNALS__), true);
    await page.setViewportSize({ width: 1280, height: 800 });
    await cdp.send('Emulation.setCPUThrottlingRate', { rate });
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    for (let index = 0; index < count; index++) {
      await page.locator('.app-navigation-home').evaluate(element => element.click());
      await page.locator('[data-surface="start"] .composer-textarea').waitFor();
      await page.waitForTimeout(800);
      await page.locator('.composer-textarea').fill('/help');
      await page.locator('.composer-textarea').press('Escape');
      await page.waitForTimeout(200);
      if (traceEnabled) {
        await cdp.send('Tracing.start', { categories: 'devtools.timeline,blink.user_timing', transferMode: 'ReturnAsStream' });
        tracing = true;
      }
      const sample = await page.evaluate(async () => {
        performance.clearMarks('start-chat:send'); performance.clearMarks('start-chat:dom');
        const start = performance.now();
        performance.mark('start-chat:send');
        let spinner = false;
        const ready = new Promise((resolve, reject) => {
          const observer = new MutationObserver(() => {
            spinner ||= !!document.querySelector('.app-main-surface > [aria-busy="true"]');
            if (!document.querySelector('[data-surface="chat"] [data-chat-scroll]')) return;
            clearTimeout(timer); observer.disconnect();
            performance.mark('start-chat:dom'); resolve(performance.now());
          });
          const timer = setTimeout(() => { observer.disconnect(); reject(Error('Chat did not mount')); }, 15000);
          observer.observe(document.querySelector('.app-main-surface'), { childList: true, subtree: true });
        });
        document.querySelector('.composer-textarea').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true }));
        const dispatch = performance.now();
        const dom = await ready;
        const frame1 = await new Promise(resolve => requestAnimationFrame(() => resolve(performance.now())));
        const frame2 = await new Promise(resolve => requestAnimationFrame(() => resolve(performance.now())));
        return { dispatchMs: dispatch - start, afterDispatchToDomMs: dom - dispatch,
          domMs: dom - start, frame1Ms: frame1 - start, frame2Ms: frame2 - start, spinner };
      });
      if (traceEnabled) {
        const trace = await endTrace(cdp); tracing = false;
        fs.writeFileSync(path.join(output, `trace-${index + 1}.json`), JSON.stringify(trace));
        sample.trace = traceSummary(trace);
      }
      assert.equal(sample.spinner, false);
      samples.push(sample);
      await page.waitForFunction(() => !document.querySelector('.composer-send-btn--stop'), undefined, { timeout: 30000 });
      await page.waitForFunction(() => !document.querySelector('.message-send-flight-root'), undefined, { timeout: 8000 });
      console.log(JSON.stringify({ sample: index + 1, rate, ...sample }));
    }
    await page.screenshot({ path: path.join(output, 'chat.png') });
    assert.deepEqual(errors, []);
  } finally {
    if (tracing) await endTrace(cdp).catch(() => {});
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: 1 }).catch(() => {});
    await page.emulateMedia({ reducedMotion: null }).catch(() => {});
    fs.writeFileSync(path.join(output, 'measurements.json'), JSON.stringify({ rate, traceEnabled, samples, errors }, null, 2));
    await browser.close();
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
