// Real React app and controls, isolated browser API fixtures. Never connects to a user's DB.
// Run with Playwright available on NODE_PATH and a Vite server on UI_AUDIT_URL.
const { chromium } = require('playwright');
const ts = require('typescript');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { buildAudit } = require('./ui-audit-build.cjs');
const production = process.argv.includes('--production');
const navigationOnly = process.argv.includes('--navigation-only');
const label = process.argv.find(arg => arg.startsWith('--label='))?.split('=')[1] || 'after';
const output = path.resolve(__dirname, '../target/ui-responsiveness', label);
fs.mkdirSync(output, { recursive: true });
const categories = ['general', 'appearance', 'config', 'models', 'personalize', 'shortcuts', 'mcp', 'hooks', 'connections', 'git', 'env', 'worktree', 'browser', 'computer', 'project_map_debug', 'archive', 'about'];

function summarize(rows) {
  const groups = {};
  for (const row of rows) {
    if (row.domMs === undefined) continue;
    const group = row.name.startsWith('provider:') ? 'provider' : row.name.replace(/:[^:]+$/, '');
    (groups[group] ??= []).push(row);
  }
  return Object.fromEntries(Object.entries(groups).map(([name, entries]) => {
    const percentile = (field, fraction) => {
      const values = entries.map(row => row[field]).sort((a, b) => a - b);
      return +values[Math.ceil(values.length * fraction) - 1].toFixed(2);
    };
    return [name, { samples: entries.length, domP50Ms: percentile('domMs', .5), domP95Ms: percentile('domMs', .95),
      domMaxMs: percentile('domMs', 1), handlerMaxMs: percentile('handlerMs', 1), within10ms: entries.filter(row => row.domMs <= 10).length }];
  }));
}

async function measure(page, name, selector, ready, input = null) {
  const result = await page.evaluate(async ({ selector, ready, input }) => {
    const target = document.querySelector(selector);
    if (!target) throw new Error(`Missing action: ${selector}`);
    const predicate = new Function(`return (${ready})`);
    const start = performance.now();
    const before = { ...window.__uiAudit.calls };
    let sawSpinner = false;
    const domMs = await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { observer.disconnect(); reject(new Error('UI readiness timeout')); }, 15000);
      const check = () => {
        sawSpinner ||= !!document.querySelector('.settings-section [aria-busy="true"], .app-main-surface > [aria-busy="true"]');
        if (!predicate()) return;
        observer.disconnect(); clearTimeout(timeout); resolve(performance.now() - start);
      };
      const observer = new MutationObserver(check);
      observer.observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
      const handlerStart = performance.now();
      if (input === null) target.click();
      else {
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(target, input);
        target.dispatchEvent(new Event('input', { bubbles: true }));
      }
      window.__uiAudit.lastHandlerMs = performance.now() - handlerStart;
      check();
    });
    const nextFrameMs = await new Promise(resolve => requestAnimationFrame(() => resolve(performance.now() - start)));
    const requests = Object.fromEntries(Object.entries(window.__uiAudit.calls).map(([key, value]) => [key, value - (before[key] || 0)]).filter(([, value]) => value));
    return { handlerMs: window.__uiAudit.lastHandlerMs, domMs, nextFrameMs, sawSpinner, requests };
  }, { selector, ready, input });
  const entry = { name, ...result };
  console.log(JSON.stringify(entry));
  return entry;
}

async function run(rate) {
  const executablePath = process.env.UI_AUDIT_BROWSER || [process.env['ProgramFiles(x86)'], process.env.ProgramFiles]
    .filter(Boolean).map(root => path.join(root, 'Microsoft/Edge/Application/msedge.exe')).find(file => fs.existsSync(file));
  const browser = await chromium.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setCPUThrottlingRate', { rate });
  const errors = [], rows = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(({ modelCount }) => {
    const providers = ['deepseek', 'layso', 'third'].map((id, index) => ({ id, channel: ['DeepSeek', 'Layso', 'Third'][index],
      logo: index < 2 ? `/providers/${index ? 'layso.webp' : 'deepseek.svg'}` : '', websiteUrl: index ? 'https://layso.ai/' : 'https://www.deepseek.com/',
      modelUrl: index ? 'https://api.layso.ai/v1' : 'https://api.deepseek.com', protocol: 'responses', balanceKind: 'none', enabled: true, hasApiKey: true, revision: 1 }));
    const models = providers.flatMap(provider => Array.from({ length: modelCount }, (_, index) => ({ providerId: provider.id,
      modelName: `${provider.id}-model-${index}`, displayName: `${provider.channel} Model ${index}`, protocol: null, contextWindow: 128000,
      outputLimit: 8192, active: index < 2, thinkingLevels: ['off', 'low', 'medium', 'high', 'max'], thinkingDistinct: [], thinkingDisable: true })));
    window.__uiAudit = { calls: {}, lastHandlerMs: 0, async invoke(command, args = {}) {
      this.calls[command] = (this.calls[command] || 0) + 1;
      await new Promise(resolve => setTimeout(resolve, 120));
      if (command === 'harness_providers_list') return providers;
      if (command === 'harness_models_list' || command === 'harness_models_fetch') return models.filter(model => model.providerId === args.providerId);
      if (command === 'harness_active_models') return [];
      if (command === 'harness_model_selection') return null;
      if (command === 'harness_model_set_active') return true;
      throw new Error(`Unexpected audit IPC: ${command}`);
    } };
    localStorage.setItem('i18nextLng', 'zh');
  }, { modelCount: Number(process.env.UI_AUDIT_MODEL_COUNT || 40) });
  if (!production) await page.route('**/src/harnessModelsApi.ts', async route => {
    const response = await route.fetch();
    const source = (await response.text()).replace(/import\s*\{\s*invoke\s*\}\s*from\s*["'][^"']+["'];?/, 'const invoke = (command, args) => window.__uiAudit.invoke(command, args);');
    await route.fulfill({ response, body: source });
  });
  if (!production) await page.route('**/src/api.ts', async route => {
    const response = await route.fetch();
    let source = await response.text();
    const ast = ts.createSourceFile('api.js', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
    const edits = [];
    for (const node of ast.statements) {
      if (!ts.isFunctionDeclaration(node) || !node.body || !node.name) continue;
      const name = node.name.text;
      if (name === 'getSettings') {
        edits.push({ start: node.body.getStart(ast), end: node.body.end, text: `{ window.__uiAudit.calls.getSettings=(window.__uiAudit.calls.getSettings||0)+1; return {configured:true,available_models:['deepseek-flash'],model_capabilities:[],chat_model:'deepseek-flash',reasoning_effort:'high',sandbox_mode:'none'}; }` });
      } else if (node.modifiers?.some(modifier => modifier.kind === ts.SyntaxKind.AsyncKeyword)) {
        edits.push({ start: node.body.getStart(ast) + 1, end: node.body.getStart(ast) + 1, text: `window.__uiAudit.calls[${JSON.stringify(name)}]=(window.__uiAudit.calls[${JSON.stringify(name)}]||0)+1;` });
      }
    }
    for (const edit of edits.sort((a, b) => b.start - a.start)) source = source.slice(0, edit.start) + edit.text + source.slice(edit.end);
    await route.fulfill({ response, body: source });
  });
  try {
    await page.goto(process.env.UI_AUDIT_URL || 'http://127.0.0.1:1420/', { waitUntil: 'networkidle', timeout: 180000 });
    await page.locator('.app-navigation-rail').waitFor({ timeout: 120000 });
    await page.waitForTimeout(500);
    const titlebar = await page.locator('#decorum-nav-row').count();
    await page.locator('.app-navigation-bottom button[aria-label="设置"]').click();
    await page.locator('[data-menu-item="settings"]').click();
    await page.locator('[data-cat="models"]').waitFor();
    for (const round of ['cold', 'warm']) {
      for (const category of navigationOnly ? [] : categories) {
        const selector = `[data-cat="${category}"]`;
        rows.push(await measure(page, `${round}:settings:${category}`, selector,
          `document.querySelector('${selector}')?.classList.contains('font-medium') && Array.from(document.querySelectorAll('.settings-section')).some(el => (el.dataset.settingsCategory ? el.dataset.settingsCategory === '${category}' : true) && !el.hidden && !el.querySelector('[data-category-pending]') && el.textContent.trim().length > 5 && !el.querySelector('[aria-busy="true"]'))`));
        await page.waitForTimeout(round === 'cold' ? 280 : 40);
      }
    }
    await page.locator('[data-cat="models"]').click();
    await page.locator('button[aria-label="选择供应商 Layso"]').waitFor();
    await page.waitForTimeout(300);
    if (process.env.UI_AUDIT_PROFILE) { await cdp.send('Profiler.enable'); await cdp.send('Profiler.start'); }
    for (let repeat = 0; repeat < (navigationOnly ? 0 : 4); repeat++) for (const provider of ['Layso', 'DeepSeek']) {
      rows.push(await measure(page, `provider:${provider}:${repeat}`, `button[aria-label="选择供应商 ${provider}"]`,
        `document.querySelector('button[aria-label="选择供应商 ${provider}"]')?.getAttribute('aria-pressed') === 'true' && !!Array.from(document.querySelectorAll('[aria-label="模型列表"] li, [aria-label="模型列表"] [role="listitem"]')).find(el => el.textContent.includes('${provider} Model 0'))`));
    }
    if (process.env.UI_AUDIT_PROFILE) {
      const { profile } = await cdp.send('Profiler.stop');
      fs.writeFileSync(path.join(output, `providers-${rate}x.cpuprofile`), JSON.stringify(profile));
    }
    for (let repeat = 0; repeat < (navigationOnly ? 0 : 3); repeat++) {
      rows.push(await measure(page, `dialog:open:${repeat}`, '.models-settings-surface > div:first-child button', `!!document.querySelector('[role="dialog"] input#harness-channel')`));
      await page.waitForTimeout(350);
      const frames = await page.evaluate(async () => {
        const frames = [], start = performance.now();
        document.querySelector('button[aria-label="关闭供应商编辑"]').click();
        await new Promise(resolve => {
          function sample() {
            const dialog = document.querySelector('[role="dialog"]');
            frames.push({ timeMs: performance.now() - start, dialog: !!dialog, form: !!dialog?.querySelector('form'), height: dialog?.getBoundingClientRect().height || 0 });
            if (dialog && performance.now() - start < 1200) requestAnimationFrame(sample); else resolve();
          }
          sample();
        });
        return frames;
      });
      rows.push({ name: `dialog:close:${repeat}`, frames, emptyShell: frames.some(frame => frame.dialog && !frame.form) });
    }
    await page.screenshot({ path: path.join(output, `models-${rate}x.png`) });
    if (!label.startsWith('before')) {
      if (!navigationOnly) {
      const modelCount = Number(process.env.UI_AUDIT_MODEL_COUNT || 40);
      const mountedRows = await page.locator('[aria-label="模型列表"] [role="listitem"]').count();
      assert.ok(mountedRows < Math.min(30, modelCount), 'large registries must render a bounded visible window');
      await page.locator('[aria-label="模型列表"]').evaluate(element => { element.scrollTop = element.scrollHeight; });
      await page.locator(`[data-model-name="deepseek-model-${modelCount - 1}"]`).waitFor();
      // Search from the bottom must reset the filtered viewport, too.
      rows.push(await measure(page, 'model:search', 'input[aria-label="搜索模型"]',
        `!!document.querySelector('[data-model-name="deepseek-model-17"]') && !document.querySelector('[data-model-name="deepseek-model-0"]')`, 'Model 17'));
      rows.push(await measure(page, 'model:clear-search', 'input[aria-label="搜索模型"]',
        `!!document.querySelector('[data-model-name="deepseek-model-0"]')`, ''));
      rows.push(await measure(page, 'model:activate', '[role="checkbox"][aria-label="激活 deepseek-model-0"]',
        `document.querySelector('[role="checkbox"][aria-label="激活 deepseek-model-0"]')?.getAttribute('data-state') === 'unchecked'`));
      rows.push(await measure(page, 'model:edit-open', 'button[aria-label="编辑 deepseek-model-0"]', `!!document.querySelector('input#model-name')`));
      rows.push(await measure(page, 'model:edit-close', 'button[aria-label="关闭模型编辑"]', `!document.querySelector('[role="dialog"]')`));
      }
      // Cached pages are exercised through their real navigation controls.
      for (const round of ['cold', 'warm']) for (const surface of ['skills', 'knowledge', 'plugins', 'automation', 'remote', 'start']) {
        let selector = surface === 'start' ? '.app-navigation-home' : `[data-nav="${surface}"]`;
        if (['plugins', 'automation', 'remote'].includes(surface)) {
          await page.locator('[data-nav="other"]').click();
          selector = `[data-nav-surface="${surface}"]`;
          await page.locator(selector).waitFor();
        }
        rows.push(await measure(page, `${round}:surface:${surface}`, selector,
          `!!Array.from(document.querySelectorAll('[data-surface="${surface}"]')).find(el => !el.hidden && el.getClientRects().length && el.textContent.trim().length > 3)`));
        await page.waitForTimeout(400);
      }
      await page.locator('.app-navigation-bottom button[aria-label="设置"]').click();
      await page.locator('[data-menu-item="settings"]').click();
      await page.locator('[data-cat="models"]').click();
      await page.waitForTimeout(150);
      // A hidden retained settings page must not leave a portal overlay behind.
      await page.locator('.models-settings-surface > div:first-child button').click();
      await page.locator('input#harness-channel').waitFor();
      await page.locator('.app-navigation-home').evaluate(element => element.click());
      await page.waitForTimeout(50);
      assert.equal(await page.locator('[role="dialog"]').count(), 0, 'hidden pages must dismiss portal dialogs');
      await page.locator('.app-navigation-bottom button[aria-label="设置"]').click();
      await page.locator('[data-menu-item="settings"]').click();
      await page.locator('[data-cat="models"]').click();
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.waitForTimeout(150);
      await page.screenshot({ path: path.join(output, `models-1280-${rate}x.png`) });
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'the layout must fit the viewport');
    }
    const calls = await page.evaluate(() => window.__uiAudit.calls);
    const result = { label, cpuRate: rate, modelCount: Number(process.env.UI_AUDIT_MODEL_COUNT || 40), titlebar, summary: summarize(rows), rows, calls, errors, mode: `${production ? 'Production' : 'Vite development'} build, real app renderer; API mock mode, registry fixture 120ms; no native/backend latency measured` };
    fs.writeFileSync(path.join(output, `measurements-${rate}x.json`), JSON.stringify(result, null, 2));
    if (!label.startsWith('before')) {
      assert.deepEqual(errors, [], 'browser interactions must not raise uncaught errors');
      assert.equal(titlebar, 1, 'navigation must exist without an injected native titlebar');
      assert.equal(rows.some(row => row.emptyShell), false, 'closing dialogs must never expose an empty shell');
      const providerRows = rows.filter(row => row.name.startsWith('provider:') && Number(row.name.split(':')[2]) > 0);
      assert.ok(providerRows.every(row => !row.requests.harness_models_list), 'warm provider switches must use cached rows');
      assert.ok(rows.filter(row => row.name.startsWith('warm:settings:')).every(row => !Object.keys(row.requests).length), 'warm settings must reuse their loaded state');
      assert.ok(rows.filter(row => /warm:surface:(skills|knowledge|plugins|automation)/.test(row.name)).every(row => !Object.keys(row.requests).length), 'retained pages must not reload their data');
    }
  } catch (error) {
    fs.writeFileSync(path.join(output, `failure-${rate}x.json`), JSON.stringify({ rows, errors, error: String(error), text: await page.locator('body').innerText() }, null, 2));
    await page.screenshot({ path: path.join(output, `failure-${rate}x.png`) });
    throw error;
  } finally { await browser.close(); }
}
(async () => {
  const server = production ? await buildAudit(label) : null;
  if (server) process.env.UI_AUDIT_URL = 'http://127.0.0.1:1421/';
  try { for (const rate of (process.env.UI_AUDIT_RATES || '1,6').split(',').map(Number)) await run(rate); }
  finally { if (server) await new Promise(resolve => server.httpServer.close(resolve)); }
})().catch(error => { console.error(error); process.exitCode = 1; });
