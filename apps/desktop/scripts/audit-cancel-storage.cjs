// Opt-in, destructive fault injection in an isolated target/ desktop test DB only.
// Start the real Tauri app with this data root and a CDP port before invoking.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { DatabaseSync } = require('node:sqlite');

async function main() {
  const root = fs.realpathSync(process.env.AUDIT_DATA_ROOT);
  const target = fs.realpathSync(path.resolve(__dirname, '../../../target'));
  const relative = path.relative(target, root);
  assert(relative && !relative.startsWith('..') && !path.isAbsolute(relative),
    'AUDIT_DATA_ROOT must be an isolated directory inside this repository target/');
  assert(fs.existsSync(path.join(root, 'deepagent.db')), 'Start the isolated desktop first');
  const { chromium } = require(process.env.AUDIT_PLAYWRIGHT_MODULE);
  const browser = await chromium.connectOverCDP(process.env.AUDIT_CDP_URL);
  const db = new DatabaseSync(path.join(root, 'deepagent.db'));
  db.exec('PRAGMA busy_timeout=3000');
  let page;
  let triggerInstalled = false;
  try {
    page = browser.contexts().flatMap(c => c.pages()).find(p => /localhost:1420|127\.0\.0\.1:1420/.test(p.url()));
    assert(page, 'Real desktop WebView on port 1420 is required');
    assert(await page.evaluate(() => !!window.__TAURI_INTERNALS__), 'Browser mock is not a desktop test');
    assert.equal(await page.locator('button[aria-label="停止"]').count(), 0, 'Do not interrupt another run');
    const before = new Set(db.prepare('SELECT id FROM runs').all().map(r => r.id));
    await page.evaluate(async () => {
      const { listen } = await import('/node_modules/@tauri-apps/api/event.js');
      window.auditStorageEvents = [];
      window.auditStorageUnlisten = await Promise.all(['chat://event', 'session://completed'].map(name => listen(name, e => {
        const p = e.payload;
        const event = p.payload ?? p;
        // Intentionally exclude provider payloads, credentials, and prompt content.
        window.auditStorageEvents.push({name, type:event.type, run_id:p.run_id, status:p.status, error:p.error, at:Date.now()});
      })));
    });
    const ipcs = [];
    const started = Date.now();
    const command = url => ['start_chat_v2', 'cancel_run'].find(n => decodeURIComponent(url).includes(n));
    page.on('request', req => { const name = command(req.url()); if (name) ipcs.push({event:'request',name,ms:Date.now()-started}); });
    page.on('response', res => { const name = command(res.url()); if (name) ipcs.push({event:'response',name,status:res.status(),ms:Date.now()-started}); });
    await page.locator('textarea').fill('桌面停止测试：直接连续输出一篇五千字的软件工程文章，不调用任何工具。');
    await page.locator('button[aria-label="发送"]').click();
    await page.waitForFunction(() => window.auditStorageEvents.some(e => e.type === 'model_first_token'), undefined, {timeout:60000});
    const runs = db.prepare('SELECT id FROM runs').all().filter(r => !before.has(r.id));
    assert.equal(runs.length, 1, 'Exactly one audit run must have been admitted');
    const runId = runs[0].id;
    // Scope the injection to this run, even within an isolated test database.
    db.exec(`CREATE TRIGGER audit_ui_fail_cancel BEFORE INSERT ON run_events WHEN NEW.run_id='${runId.replaceAll("'", "''")}' AND NEW.event_type='cancel.requested' BEGIN SELECT RAISE(ABORT,'audit injected cancel audit storage failure'); END;`);
    triggerInstalled = true;
    const stopAt = Date.now();
    await page.locator('button[aria-label="停止"]').click();
    await page.waitForFunction(() => window.auditStorageEvents.some(e => e.name === 'session://completed'), undefined, {timeout:15000});
    await page.waitForTimeout(1200);
    const ui = await page.evaluate(() => ({
      events:window.auditStorageEvents,
      visibleStorageError:document.body.innerText.includes('audit injected cancel audit storage failure'),
      sendVisible:[...document.querySelectorAll('button[aria-label="发送"]')].some(e=>e.getBoundingClientRect().width>0),
      stopVisible:[...document.querySelectorAll('button[aria-label="停止"]')].some(e=>e.getBoundingClientRect().width>0),
    }));
    const run = db.prepare('SELECT id,state,terminal_kind,finished_at FROM runs WHERE id=?').get(runId);
    const events = db.prepare('SELECT sequence,event_type FROM run_events WHERE run_id=? ORDER BY sequence').all(runId);
    const result = {ipcs,stopAt,ui,run,events};
    fs.writeFileSync(path.join(root,'cancel-storage-failure.json'),JSON.stringify(result,null,2));
    await page.screenshot({path:path.join(root,'cancel-storage-failure.png')});
    console.log(JSON.stringify(result,null,2));
    assert(ui.events.some(e => e.type === 'run_cancelled'), 'Cancellation must reach the runtime');
    assert(ui.events.some(e => e.name === 'session://completed' && e.status === 'failed' && e.error?.includes('audit injected')), 'Persistence failure must reach the desktop');
    assert.equal(run.finished_at, null, 'Faulted transaction must not claim a saved terminal');
    // Expected to fail until the UI distinguishes execution and persistence outcomes.
    assert(ui.visibleStorageError, 'H-17: desktop silently discarded the terminal persistence error');
  } finally {
    if (triggerInstalled) db.exec('DROP TRIGGER audit_ui_fail_cancel');
    db.close();
    if (page) await page.evaluate(() => {
      window.auditStorageUnlisten?.forEach(f=>f());
      delete window.auditStorageUnlisten;
      delete window.auditStorageEvents;
    }).catch(()=>{});
    await browser.close();
  }
}

main().catch(error => {console.error(error.message);process.exitCode=1;});
