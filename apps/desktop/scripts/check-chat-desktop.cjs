// Native Tauri WebView2 only. Connect to an explicitly launched debug port.
// Never prints IPC arguments, user messages, credentials or model response text.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const output = path.resolve(process.env.DESKTOP_AUDIT_OUTPUT || '../../target/chat-ui-audit-20261118/native');
fs.mkdirSync(output, { recursive: true });
const endpoint = process.env.DESKTOP_AUDIT_CDP || 'http://127.0.0.1:9223';
const click = locator => locator.evaluate(element => element.click());
const frames = page => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));

async function main() {
  const browser = await chromium.connectOverCDP(endpoint, { timeout: 60000 });
  const page = browser.contexts().flatMap(context => context.pages()).find(page => !page.url().includes('splash'));
  assert.ok(page, 'a native main WebView must exist');
  const errors = [], result = {};
  result.cpuRate = Number(process.env.DESKTOP_AUDIT_CPU_RATE || 1);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Emulation.setCPUThrottlingRate', { rate: result.cpuRate });
  let previous;
  const invoke = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  page.on('pageerror', error => errors.push(error.message));
  try {
    await page.locator('.app-navigation-rail').waitFor({ timeout: 120000 });
    assert.equal(await page.evaluate(() => !!window.__TAURI_INTERNALS__), true);
    await page.evaluate(async () => {
      window.__desktopAudit?.dispose?.();
      window.__desktopAudit = { events: [] };
      const unlisten = await window.__TAURI__.event.listen('chat://event', event => {
        const payload = event.payload?.payload || event.payload;
        window.__desktopAudit.events.push({ type: payload.type, at: performance.now() });
      });
      window.__desktopAudit.dispose = unlisten;
    });
    previous = await invoke('harness_model_selection');
    const active = await invoke('harness_active_models');
    assert.ok(active.length, 'native test needs configured, activated models');
    await click(page.locator('.app-navigation-home'));
    await frames(page);
    await page.locator('.composer-textarea').waitFor();
    await page.locator('.composer-textarea').fill('/model');
    await page.locator('[role="option"]').first().waitFor();
    const choices = await page.locator('[data-composer-suggest] [role="option"]').allTextContents();
    result.slashChoiceCount = choices.length;
    assert.equal(choices.length, active.length, 'slash lists activated routes, not discovered models');
    for (const model of active) assert.ok(choices.some(text => text.includes(model.modelName) && text.includes(model.providerChannel)));
    assert.ok(choices.every(text => !text.includes('这个 DeepSeek')));
    await page.locator('.composer-textarea').fill('/');
    await page.waitForFunction(() => [...document.querySelectorAll('[data-composer-suggest] [role="option"]')].some(element => element.textContent.includes('/help')));
    const scroll = page.locator('.composer-suggest-scroll');
    const bounds = await scroll.boundingBox();
    await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + Math.min(60, bounds.height / 2));
    const positions = [];
    for (let index = 0; index < 6; index++) {
      await page.mouse.wheel(0, 60);
      await page.waitForTimeout(100);
      positions.push(await scroll.evaluate(element => element.scrollTop));
    }
    result.slashWheelPositions = positions;
    assert.ok(positions.at(-1) > 0, 'wheel must advance the slash menu');
    assert.ok(positions.every((top, index) => index === 0 || top >= positions[index - 1] - 1), 'wheel must not bounce back');
    await page.locator('.composer-textarea').press('ArrowUp');
    await frames(page);
    result.keyboardScrollTop = await scroll.evaluate(element => element.scrollTop);
    assert.equal(await scroll.evaluate(element => {
      const selected = element.querySelector('[aria-selected="true"]')?.getBoundingClientRect();
      const viewport = element.getBoundingClientRect();
      return !!selected && selected.top >= viewport.top - 1 && selected.bottom <= viewport.bottom + 1;
    }), true, 'keyboard navigation must keep the selected row visible');
    await page.locator('.composer-textarea').fill('');

    // A local slash command exercises the real Start -> Chat admission without inference.
    const selected = active.find(model => model.modelName === process.env.DESKTOP_AUDIT_MODEL) || active.find(model => model.providerId !== previous?.providerId) || active[0];
    const qualified = active.filter(model => model.modelName === selected.modelName).length > 1;
    const command = `/model ${qualified ? `${selected.providerId}/` : ''}${selected.modelName}`;
    result.slashUsesProviderPrefix = qualified;
    await page.locator('.composer-textarea').fill(command);
    await page.locator('.composer-textarea').press('Escape');
    if (process.env.DESKTOP_AUDIT_PROFILE === '1') {
      await cdp.send('Profiler.enable');
      await cdp.send('Profiler.start');
    }
    result.startToChat = await page.evaluate(async () => {
      const start = performance.now();
      let spinner = false;
      const ready = new Promise((resolve, reject) => {
        const timeout = setTimeout(() => { observer.disconnect(); reject(new Error('chat shell did not mount within 15s')); }, 15000);
        const observer = new MutationObserver(() => {
          spinner ||= !!document.querySelector('.app-main-surface > [aria-busy="true"]');
          if (!document.querySelector('[data-surface="chat"] [data-chat-scroll]')) return;
          clearTimeout(timeout); observer.disconnect(); resolve(performance.now() - start);
        });
        observer.observe(document.querySelector('.app-main-surface'), { childList: true, attributes: true, subtree: true });
      });
      document.querySelector('.composer-textarea').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', bubbles: true }));
      const domMs = await ready;
      return { domMs, spinner };
    });
    if (process.env.DESKTOP_AUDIT_PROFILE === '1') {
      const { profile } = await cdp.send('Profiler.stop');
      fs.writeFileSync(path.join(output, 'start-to-chat.cpuprofile'), JSON.stringify(profile));
    }
    assert.equal(result.startToChat.spinner, false, 'chat shell must not lazy-load on send');
    await page.waitForFunction(() => document.querySelector('.composer-textarea') && !document.querySelector('button[aria-label="停止"]'), undefined, { timeout: 15000 });
    await page.waitForFunction(async ({ providerId, modelName }) => {
      const selection = await window.__TAURI_INTERNALS__.invoke('harness_model_selection');
      return selection?.providerId === providerId && selection?.modelName === modelName;
    }, selected, { timeout: 15000 });
    result.switchedModel = { providerId: selected.providerId, modelName: selected.modelName };
    await page.screenshot({ path: path.join(output, 'slash-switch.png') });

    if (process.env.DESKTOP_AUDIT_GREETING === '1') {
      if (previous) await invoke('harness_model_select', { providerId: previous.providerId, modelName: previous.modelName });
      await click(page.locator('.app-navigation-home'));
      await frames(page);
      await page.locator('.composer-textarea').fill('你好');
      await page.evaluate(() => { window.__desktopAudit.greetingStart = performance.now(); });
      await page.locator('.composer-textarea').press('Enter');
      await page.waitForFunction(() => window.__desktopAudit.events.some(event =>
        event.at >= window.__desktopAudit.greetingStart && ['run_completed', 'run_failed', 'run_cancelled'].includes(event.type)
      ), undefined, { timeout: 240000 });
      result.greeting = await page.evaluate(() => ({
        durationMs: performance.now() - window.__desktopAudit.greetingStart,
        events: window.__desktopAudit.events.filter(event => event.at >= window.__desktopAudit.greetingStart)
          .map(event => ({ type: event.type, atMs: event.at - window.__desktopAudit.greetingStart })),
      }));
      await page.screenshot({ path: path.join(output, 'greeting.png') });
    }

    await click(page.locator('.app-navigation-bottom button[aria-label="设置"]'));
    await click(page.locator('[data-menu-item="settings"]'));
    if (!await page.locator('[data-cat="models"]').count()) await click(page.locator('[data-sidebar-toggle="rail"]'));
    await click(page.locator('[data-cat="models"]'));
    await page.locator('[aria-label="模型列表"]').waitFor();
    await page.waitForFunction(() => !document.querySelector('[data-settings-category="models"] [aria-busy="true"]'));
    const providers = await invoke('harness_providers_list');
    const largest = (await Promise.all(providers.map(async provider => ({ provider, count: (await invoke('harness_models_list', { providerId: provider.id })).length })))).sort((a, b) => b.count - a.count)[0];
    await click(page.getByRole('button', { name: `选择供应商 ${largest.provider.channel}`, exact: true }));
    for (const size of [{ width: 1280, height: 800 }, { width: 960, height: 600 }]) {
      await page.setViewportSize(size);
      await frames(page);
      const geometry = await page.evaluate(() => {
        const metrics = selector => { const element = document.querySelector(selector); return { height: element.clientHeight, content: element.scrollHeight, top: element.scrollTop }; };
        return { outer: metrics('[data-settings-viewport]'), models: metrics('[aria-label="模型列表"]'), providers: metrics('[data-provider-scroll]') };
      });
      assert.ok(geometry.outer.content <= geometry.outer.height + 1, 'model settings must fit the viewport');
      assert.ok(geometry.models.height > 60 && geometry.providers.height > 60, 'both list viewports must have usable height');
      const list = page.locator('[aria-label="模型列表"]');
      const box = await list.boundingBox();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.wheel(0, 800);
      await page.waitForTimeout(150);
      assert.equal(await page.locator('[data-settings-viewport]').evaluate(element => element.scrollTop), 0);
      const modelTop = await list.evaluate(element => element.scrollTop);
      if (geometry.models.content > geometry.models.height) assert.ok(modelTop > 0, 'model list must scroll internally');
      const providerList = page.locator('[data-provider-scroll]');
      const providerBox = await providerList.boundingBox();
      await page.mouse.move(providerBox.x + providerBox.width / 2, providerBox.y + providerBox.height / 2);
      await page.mouse.wheel(0, 800);
      await page.waitForTimeout(150);
      const providerTop = await providerList.evaluate(element => element.scrollTop);
      if (geometry.providers.content > geometry.providers.height) assert.ok(providerTop > 0, 'provider list must scroll internally');
      assert.equal(await page.locator('[data-settings-viewport]').evaluate(element => element.scrollTop), 0);
      geometry.modelTopAfterWheel = modelTop;
      geometry.providerTopAfterWheel = providerTop;
      result[`layout${size.width}`] = geometry;
      await page.screenshot({ path: path.join(output, `models-${size.width}.png`) });
    }
    result.errors = errors;
    result.events = await page.evaluate(() => window.__desktopAudit.events);
    assert.deepEqual(errors, []);
  } catch (error) {
    result.failure = error.message;
    await page.screenshot({ path: path.join(output, 'failure.png') }).catch(() => {});
    throw error;
  } finally {
    if (previous) await invoke('harness_model_select', { providerId: previous.providerId, modelName: previous.modelName }).catch(error => { result.restoreError = error.message; });
    await page.evaluate(() => window.__desktopAudit?.dispose?.()).catch(() => {});
    await cdp.send('Emulation.setCPUThrottlingRate', { rate: 1 }).catch(() => {});
    fs.writeFileSync(path.join(output, 'measurements.json'), JSON.stringify(result, null, 2));
    await browser.close();
  }
  assert.equal(result.restoreError, undefined, 'the original model selection must be restored');
  const { events, greeting, ...summary } = result;
  console.log(JSON.stringify({ ...summary, greetingMs: greeting?.durationMs }));
}
main().catch(error => { console.error(error); process.exitCode = 1; });
