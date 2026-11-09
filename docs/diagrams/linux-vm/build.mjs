import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

// Documentation tooling only. Uses explicitly supplied, already installed modules.
const options = Object.fromEntries(process.argv.slice(2).reduce((pairs, item, i, args) => {
  if (item.startsWith('--')) pairs.push([item.slice(2), args[i + 1]]);
  return pairs;
}, []));
for (const required of ['playwright', 'mermaid', 'browser', 'evidence-dir']) {
  if (!options[required]) throw new Error(`Missing --${required}; see README.md`);
}
const directory = path.dirname(fileURLToPath(import.meta.url));
const evidenceDirectory = path.resolve(options['evidence-dir']);
await fs.mkdir(evidenceDirectory, { recursive: true });
const { chromium } = await import(pathToFileURL(path.resolve(options.playwright)).href);
const diagrams = [
  ['architecture', '01', '部署与信任边界', '哪些进程在宿主、哪些在 Linux；权限、数据与控制的权威分别在哪里。', 'I01–I08 · UP-00 / 01 / 04 / 05'],
  ['components', '02', '源码组件改造', '运行准备、宿主 SDK 注入、严格 code 与后端选择的真实改造位置。', 'UP-01 / 05 / 07 / 09'],
  ['execution', '03', '命令完整时序', '原审批、空间租约、进程、输出、取消和唯一运行终态保持一条链。', 'I03 / I04 / I07 / I08'],
  ['workspace', '04', '文件导入与提交', '宿主是权威副本；Linux 只提出变更；并发编辑与部分提交必须可恢复。', 'I05 · UP-06'],
  ['storage', '05', '存储与所有权', '3,000 MB 全局预算、物理文件计量、用户文件所有权与 guest 内部空间。', 'I08 · UP-02 / 03'],
  ['maintenance', '06', '清理与恢复', '定时、压力和手动清理共用原归属、引用、资源 claim 与删除恢复链。', 'I09 · UP-10'],
  ['lifecycle', '07', '启动与运行状态', '前端先就绪；后台探测、激活与启动；失败可诊断，scope 切换重建。', 'UP-00 / 04 / 11'],
];
const escape = value => String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
const browser = await chromium.launch({ executablePath: path.resolve(options.browser), headless: true });
const results = [];
const rendered = [];
try {
  const page = await browser.newPage({ viewport: { width: 1800, height: 1100 }, deviceScaleFactor: 1 });
  await page.setContent('<!doctype html><html lang="zh-CN"><meta charset="utf-8"><style>body{margin:24px;background:white;font-family:"Microsoft YaHei",sans-serif}#diagram{display:inline-block}svg{max-width:none!important}</style><div id="diagram"></div></html>');
  await page.addScriptTag({ path: path.resolve(options.mermaid) });
  await page.evaluate(() => window.mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    theme: 'base',
    fontFamily: 'Microsoft YaHei, sans-serif',
    themeVariables: {
      fontSize: '15px', primaryColor: '#eff6ff', primaryTextColor: '#0f172a',
      primaryBorderColor: '#64748b', lineColor: '#64748b', secondaryColor: '#f1f5f9',
      tertiaryColor: '#ffffff', clusterBkg: '#f8fafc', clusterBorder: '#cbd5e1',
      actorBkg: '#eff6ff', actorBorder: '#64748b', actorTextColor: '#0f172a',
      signalColor: '#475569', signalTextColor: '#0f172a', noteBkgColor: '#fffbeb',
      noteBorderColor: '#d97706', noteTextColor: '#78350f', activationBkgColor: '#dbeafe',
    },
    flowchart: { htmlLabels: true, curve: 'linear', nodeSpacing: 28, rankSpacing: 48, padding: 16, useMaxWidth: false },
    sequence: { useMaxWidth: false, wrap: true, width: 150, actorMargin: 38, messageMargin: 30, noteMargin: 14, boxMargin: 10, diagramMarginX: 24 },
  }));
  for (const [name, number, title, description, tags] of diagrams) {
    const source = await fs.readFile(path.join(directory, `${name}.mmd`), 'utf8');
    const result = await page.evaluate(async ({ source, name, title }) => {
      await window.mermaid.parse(source);
      const { svg } = await window.mermaid.render(`vm-${name}`, source);
      const container = document.getElementById('diagram');
      container.innerHTML = svg;
      const element = container.querySelector('svg');
      const bounds = element.viewBox.baseVal;
      element.setAttribute('width', String(bounds.width));
      element.setAttribute('height', String(bounds.height));
      element.setAttribute('aria-label', title);
      const text = document.createElementNS('http://www.w3.org/2000/svg', 'title');
      text.textContent = title;
      element.prepend(text);
      return { svg: new XMLSerializer().serializeToString(element), width: bounds.width, height: bounds.height,
        nodes: element.querySelectorAll('.node, .actor, .stateGroup').length,
        labels: element.querySelectorAll('.nodeLabel, .messageText, .state-title').length };
    }, { source, name, title });
    await fs.writeFile(path.join(directory, `${name}.svg`), result.svg);
    await page.evaluate(() => {
      const svg = document.querySelector('#diagram svg');
      svg.style.width = `${Math.min(svg.viewBox.baseVal.width, 1800)}px`;
      svg.style.height = 'auto';
    });
    await page.locator('#diagram').screenshot({ path: path.join(evidenceDirectory, `${name}.png`) });
    results.push({ name, title, parser: 'installed Mermaid', width: result.width, height: result.height,
      nodes: result.nodes, labels: result.labels, svgBytes: Buffer.byteLength(result.svg) });
    rendered.push({ name, number, title, description, tags, ...result });
  }
  const nav = rendered.map(d => `<a href="#${d.name}"><span>${d.number}</span>${escape(d.title)}</a>`).join('');
  const sections = rendered.map(d => `<article id="${d.name}" data-native-width="${d.width}">
    <header><div><span class="eyebrow">VIEW ${d.number} · ${escape(d.tags)}</span><h2>${escape(d.title)}</h2><p>${escape(d.description)}</p></div>
    <div class="controls"><button data-action="fit">适应宽度</button><button data-action="minus" aria-label="缩小">−</button><button data-action="plus" aria-label="放大">＋</button><button data-action="native">100%</button><button data-action="expand">展开</button><a href="${d.name}.svg">SVG</a><a href="${d.name}.mmd">图源</a></div></header>
    <div class="diagram">${d.svg}</div><footer>R 保留　M 扩展　N 新增　G 待验证　<span>可缩放并横向滚动 · 未来设计，不表示已实现</span></footer></article>`).join('');
  const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>DeepAgent Studio · Linux 升级架构图册 v2</title>
<style>
*{box-sizing:border-box}html{scroll-behavior:smooth}body{margin:0;background:#f3f5f8;color:#172033;font-family:"Segoe UI","Microsoft YaHei",sans-serif;line-height:1.6}a{color:#1d4ed8;text-decoration:none}button{font:inherit;cursor:pointer}aside{position:fixed;inset:0 auto 0 0;width:238px;padding:30px 20px;background:#101b2d;color:#cbd5e1;overflow:auto}aside strong{display:block;color:#fff;font-size:18px;line-height:1.4}aside small{display:block;margin:12px 0 28px;font-size:12px;color:#94a3b8}nav a{display:flex;gap:12px;color:#cbd5e1;font-size:14px;padding:12px 8px;border-radius:5px}nav a:hover{background:#22324b;color:#fff}nav span{font-variant-numeric:tabular-nums;color:#7891b4}aside .doc{display:block;border-top:1px solid #344155;margin-top:28px;padding-top:20px;color:#b7d5ff;font-size:13px}main{margin-left:238px;padding:40px 36px 60px;max-width:2400px}.page-head{max-width:1100px;margin-bottom:30px}.eyebrow{font-size:11px;letter-spacing:1.1px;font-weight:700;color:#52657f}h1{font-size:31px;letter-spacing:-.6px;margin:10px 0 12px;line-height:1.3}h2{font-size:21px;margin:6px 0}p{margin:0;color:#536176;font-size:14px}.notice{padding:14px 18px;background:#fffbeb;border:1px solid #f1d18a;border-left:4px solid #d97706;margin:20px 0;font-size:13px;color:#713f12}.legend{display:flex;flex-wrap:wrap;gap:18px;font-size:13px;margin-top:18px}.legend i{display:inline-block;width:11px;height:11px;margin-right:7px;border:1px solid;vertical-align:middle}.r{background:#f1f5f9;border-color:#64748b!important}.m{background:#eff6ff;border-color:#2563eb!important}.n{background:#ecfdf5;border-color:#059669!important}.g{background:#fffbeb;border-color:#d97706!important}article{background:#fff;border:1px solid #dbe2eb;border-radius:8px;margin:0 0 28px;scroll-margin-top:20px;overflow:hidden}article header{display:flex;justify-content:space-between;align-items:flex-start;gap:20px;padding:23px 25px;border-bottom:1px solid #e5eaf1}.controls{display:flex;flex-wrap:wrap;align-items:center;gap:6px;max-width:350px;flex-shrink:0}.controls button,.controls a{border:1px solid #cbd5e1;border-radius:4px;background:#fff;color:#334155;padding:4px 9px;font-size:12px}.controls button:hover,.controls a:hover{background:#eff6ff;border-color:#93b3e8}.diagram{padding:25px;overflow:auto;max-height:760px;background:#fff}.diagram svg{display:block;max-width:none!important;height:auto}.expanded .diagram{max-height:none}article footer{font-size:11px;padding:10px 25px;border-top:1px solid #e5eaf1;color:#64748b;display:flex;justify-content:space-between}.closing{max-width:1100px;font-size:13px;color:#64748b}@media(max-width:1000px){aside{position:relative;width:auto;padding:20px}aside small,aside .doc{display:none}nav{display:flex;overflow:auto;white-space:nowrap;margin-top:12px}nav a{padding:8px}main{margin-left:0;padding:24px 16px}article header{display:block}.controls{margin-top:14px}article footer span{display:none}}@media print{aside,.controls{display:none}main{margin:0;padding:0}article{break-before:page;border:0}.diagram{overflow:visible;max-height:none}article svg{width:100%!important}body{background:white}h1{font-size:24px}}
</style></head><body><aside><strong>DeepAgent Studio<br>Linux 升级架构</strong><small>ENGINEERING BLUEPRINT · v2<br>2026-11-09 · 未来设计</small><nav>${nav}</nav><a class="doc" href="../../linux-vm-implementation-blueprint.md">阅读 UP-00～UP-12 实施蓝图 ↗</a></aside><main><div class="page-head"><span class="eyebrow">ARCHITECTURE REVIEW / 2026-11-09</span><h1>从现有内核到受控 Linux 执行环境</h1><p>七个视图分开说明部署、源码、执行、文件、空间与生命周期。原运行内核、审批、取消、事件和宿主能力继续复用。</p><div class="notice"><strong>发布门槛尚未通过：</strong>目标无 WSL 的 Windows 电脑启动失败根因未闭环；双向 guest 通道、只读基盘、非 root、3 GB 硬上限仍需生产验收。图中新增组件与接口均为本地设计。</div><div class="legend"><span><i class="r"></i>R · 保留</span><span><i class="m"></i>M · 扩展现有</span><span><i class="n"></i>N · 新增</span><span><i class="g"></i>G · 待验证门槛</span></div></div>${sections}<p class="closing">设计权威：实施蓝图固定改造与依赖，接口附录固定拟议契约，证据索引固定事实与验收状态。命令完成不等于整个 run 完成；guest 报告不等于宿主授权或实际文件提交。</p></main>
<script>
const fit = article => { const box=article.querySelector('.diagram'); article.dataset.scale=String(Math.min(1,(box.clientWidth-50)/Number(article.dataset.nativeWidth))); update(article); };
const update = article => { article.querySelector('svg').style.width=(Number(article.dataset.nativeWidth)*Number(article.dataset.scale))+'px'; };
document.querySelectorAll('article').forEach(article => { fit(article); article.addEventListener('click', event => { const button=event.target.closest('button[data-action]'); if(!button)return; const action=button.dataset.action; if(action==='fit'){fit(article);return;} if(action==='expand'){const expanded=article.classList.toggle('expanded');button.textContent=expanded?'收起':'展开';return;} const old=Number(article.dataset.scale);article.dataset.scale=String(action==='native'?1:Math.max(.15,Math.min(2.5,old+(action==='plus'?.2:-.2))));update(article); }); });
</script></body></html>`;
  await fs.writeFile(path.join(directory, 'index.html'), html);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(pathToFileURL(path.join(directory, 'index.html')).href);
  await page.setViewportSize({ width: 1600, height: 1100 });
  await page.reload();
  const controls = page.locator('#architecture .controls');
  const svg = page.locator('#architecture svg');
  const original = await svg.evaluate(element => element.getBoundingClientRect().width);
  await controls.getByRole('button', { name: '放大', exact: true }).click();
  const zoomed = await svg.evaluate(element => element.getBoundingClientRect().width);
  if (!(zoomed > original)) throw new Error('Gallery zoom did not change diagram width');
  await controls.getByRole('button', { name: '适应宽度', exact: true }).click();
  await controls.getByRole('button', { name: '展开', exact: true }).click();
  if (!(await page.locator('#architecture').getAttribute('class')).includes('expanded')) throw new Error('Expand failed');
  await controls.getByRole('button', { name: '收起', exact: true }).click();
  await page.screenshot({ path: path.join(evidenceDirectory, 'atlas-desktop.png'), fullPage: false });
  await page.setViewportSize({ width: 900, height: 900 });
  await page.reload();
  await page.screenshot({ path: path.join(evidenceDirectory, 'atlas-compact.png'), fullPage: false });
  if (errors.length) throw new Error(errors.join('\n'));
  const validation = { diagrams: results, gallery: { sections: await page.locator('article').count(), zoom: 'passed', expand: 'passed', pageErrors: errors, offline: true },
    inputs: { ...options, mermaidVersion: JSON.parse(await fs.readFile(path.join(path.dirname(path.resolve(options.mermaid)), '..', 'package.json'), 'utf8')).version,
      playwrightVersion: JSON.parse(await fs.readFile(path.join(path.dirname(path.resolve(options.playwright)), 'package.json'), 'utf8')).version,
      browserVersion: await browser.version() }, evidenceDirectory };
  await fs.writeFile(path.join(evidenceDirectory, 'render-validation.json'), JSON.stringify(validation, null, 2) + '\n');
  console.log(JSON.stringify(validation, null, 2));
} finally {
  await browser.close();
}
