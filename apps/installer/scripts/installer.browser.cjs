// Visual/interaction QA against the real React UI with an explicitly synthetic
// IPC boundary. This never invokes NSIS or touches user registry/data.
const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const path = require("node:path");
const fs = require("node:fs");
const output = path.resolve(__dirname, "../.tmp/browser");
fs.mkdirSync(output, { recursive: true });
const stub = `
const f = window.fixture;
export const checkEnvironment = async (directory) => ({directory:directory ?? 'G:\\\\隔离验收\\\\DeepAgent Studio', available_bytes:100*1024**3, required_bytes:700*1024**2, webview_version:'143.0.1.0', installed:null, target_version:'0.0.8',payload_available:!f.noPayload});
export const chooseDirectory = async () => 'G:\\\\自定义位置\\\\DeepAgent Studio';
export const onProgress = async callback => { f.progress = callback; return () => f.progress = null; };
export const install = async options => { f.installs.push(options); f.progress({phase:'installing',percent:32,written_bytes:200*1024**2,total_bytes:636*1024**2}); await new Promise(resolve => f.release = resolve); if(f.failInstall) throw new Error('fixture: NSIS 退出码 7'); };
export const applyOptions = async () => { f.applies++; if (f.failOptions) throw new Error('fixture: 选项写入失败'); f.progress({phase:'complete',percent:100,written_bytes:636*1024**2,total_bytes:636*1024**2}); };
export const launchApp = async () => { f.launches++; if(f.failLaunch) throw new Error('fixture: 主程序启动失败'); f.handedOff = true; };
export const closeInstaller = async () => { f.closed = true; };
export const minimize = async () => {};
`;
async function pageFor(browser, settings = {}) {
  const page = await browser.newPage({ viewport: {width:760,height:480} });
  const errors = []; page.on("pageerror", e => errors.push(e.message));
  await page.addInitScript(settings => window.fixture = { installs:[], applies:0, launches:0, ...settings }, settings);
  await page.route("**/src/api.ts", route => route.fulfill({ contentType:"application/javascript", body:stub }));
  await page.goto(process.env.INSTALLER_TEST_URL || "http://127.0.0.1:1421/", {waitUntil:"networkidle"});
  return {page, errors};
}
async function screenshot(page, name) {
  await page.waitForTimeout(450);
  assert.ok(await page.evaluate(() => document.documentElement.scrollHeight <= innerHeight), "fixed window must not clip vertically");
  await page.screenshot({path:path.join(output,name+".png")});
}
(async () => {
  const browser = await chromium.launch({channel:"chrome",headless:true});
  const results = {};
  try {
    const {page,errors} = await pageFor(browser, {failOptions:true,failLaunch:true});
    await page.getByRole("button", {name:"立即安装",exact:true}).waitFor();
    await screenshot(page,"welcome");
    await page.getByRole("button", {name:"自定义安装"}).click();
    await page.getByRole("heading", {name:"自定义安装"}).waitFor();
    assert.equal(await page.getByRole("checkbox", {name:/登录 Windows/}).isChecked(),false);
    await page.getByRole("checkbox", {name:/加入 PATH/}).focus();
    await page.keyboard.press("Space");
    assert.equal(await page.getByRole("checkbox", {name:/加入 PATH/}).isChecked(),true);
    await page.getByRole("button", {name:"浏览",exact:true}).click();
    await screenshot(page,"custom");
    await page.getByRole("button", {name:"开始安装",exact:true}).click();
    await page.waitForFunction(() => window.fixture.installs.length === 1);
    assert.equal(await page.getByRole("button",{name:"关闭安装程序"}).isDisabled(),true);
    await screenshot(page,"installing");
    await page.evaluate(() => window.fixture.release());
    await page.getByRole("button",{name:"重试应用选项"}).waitFor();
    await page.evaluate(() => window.fixture.failOptions = false);
    await page.getByRole("button",{name:"重试应用选项"}).click();
    await page.getByRole("button",{name:"立即体验",exact:true}).waitFor();
    assert.equal(await page.evaluate(() => window.fixture.installs.length),1);
    assert.equal(await page.evaluate(() => window.fixture.applies),2);
    await screenshot(page,"done");
    await page.getByRole("button",{name:"立即体验",exact:true}).click();
    await page.getByRole("alert").filter({hasText:"fixture: 主程序启动失败"}).waitFor();
    assert.equal(await page.evaluate(() => Boolean(window.fixture.closed)),false);
    await page.evaluate(() => window.fixture.failLaunch = false);
    await page.getByRole("button",{name:"立即体验",exact:true}).click();
    await page.waitForFunction(() => window.fixture.handedOff);
    assert.deepEqual(errors,[]);
    results.success = {installCalls:1,optionRetries:1,launchFailureRetainsWindow:true,keyboardCheckbox:true,pageErrors:errors};
    await page.close();
    const failed = await pageFor(browser,{failInstall:true});
    await failed.page.getByRole("button",{name:"立即安装",exact:true}).click();
    await failed.page.waitForFunction(() => window.fixture.release);
    await failed.page.evaluate(() => window.fixture.release());
    await failed.page.getByRole("button",{name:"重试安装",exact:true}).waitFor();
    assert.equal(await failed.page.getByRole("button",{name:"立即体验",exact:true}).count(),0);
    assert.equal(await failed.page.evaluate(() => window.fixture.applies),0);
    results.failure = {neverCompleted:true}; await failed.page.close();
    const preview = await pageFor(browser,{noPayload:true});
    assert.equal(await preview.page.getByRole("button",{name:"立即安装",exact:true}).isDisabled(),true);
    results.noPayload = {installationDisabled:true}; await preview.page.close();
    fs.writeFileSync(path.join(output,"result.json"),JSON.stringify(results,null,2));
    console.log(JSON.stringify(results));
  } finally { await browser.close(); }
})().catch(e => { console.error(e);process.exitCode=1; });
