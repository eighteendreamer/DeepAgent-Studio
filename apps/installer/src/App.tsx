import { useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, Check, CheckCircle2, ChevronRight, CircleAlert, FolderOpen, HardDrive, Minus, ShieldCheck, X } from "lucide-react";
import { Button } from "../../desktop/src/components/shadcn/button";
import { Input } from "../../desktop/src/components/shadcn/input";
import { Label } from "../../desktop/src/components/shadcn/label";
import { Checkbox } from "../../desktop/src/components/shadcn/checkbox";
import { Progress } from "../../desktop/src/components/shadcn/progress";
import * as api from "./api";
import type { Environment, InstallOptions, InstallProgress } from "./api";
import brand from "./brand.svg";

type Page = "welcome" | "custom" | "installing" | "done";
const defaults: InstallOptions = { directory: "", desktop_shortcut: true, start_menu_shortcut: true, add_to_path: false, autostart: false };
const bytes = (value: number) => value >= 1024 ** 3 ? `${(value / 1024 ** 3).toFixed(1)} GB` : `${Math.ceil(value / 1024 ** 2)} MB`;
const phaseText: Record<InstallProgress["phase"], string> = { preparing: "正在准备安装包", installing: "正在安装 DeepAgent Studio", verifying: "正在校验安装文件", options: "正在应用安装选项", complete: "安装完成" };

function Option({ name, title, hint, options, setOptions }: { name: keyof Omit<InstallOptions, "directory">; title: string; hint: string; options: InstallOptions; setOptions: (value: InstallOptions) => void }) {
  return <Label htmlFor={name} className="option-row"><Checkbox id={name} checked={options[name]} onCheckedChange={checked => setOptions({ ...options, [name]: checked === true })} /><span><strong>{title}</strong><small>{hint}</small></span></Label>;
}

export function App() {
  const [page, setPage] = useState<Page>("welcome");
  const [environment, setEnvironment] = useState<Environment | null>(null);
  const [options, setOptions] = useState(defaults);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [checking, setChecking] = useState(true);
  const [installedFiles, setInstalledFiles] = useState(false);
  const [progress, setProgress] = useState<InstallProgress>({ phase: "preparing", percent: 0, written_bytes: 0, total_bytes: 0 });
  const request = useRef(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const operation = useRef(false);

  useEffect(() => {
    let active = true;
    api.checkEnvironment().then(value => {
      if (!active) return;
      setEnvironment(value); setOptions(current => ({ ...current, directory: value.directory }));
    }).catch(e => { if (active) setError(String(e)); }).finally(() => { if (active) setChecking(false); });
    return () => { active = false; };
  }, []);
  useEffect(() => { heading.current?.focus(); }, [page]);

  async function inspect(directory = options.directory) {
    const revision = ++request.current;
    setChecking(true); setError("");
    try { const next = await api.checkEnvironment(directory); if (revision === request.current) setEnvironment(next); }
    catch (e) { if (revision === request.current) { setEnvironment(null); setError(String(e)); } }
    finally { if (revision === request.current) setChecking(false); }
  }
  async function browse() {
    try { const directory = await api.chooseDirectory(); if (directory) { setOptions(current => ({ ...current, directory })); await inspect(directory); } }
    catch (e) { setError(String(e)); }
  }
  async function begin() {
    if (operation.current) return;
    operation.current = true; setBusy(true); setError(""); setPage("installing");
    let unlisten: (() => void) | undefined;
    try {
      // Subscribe before installation so the first event cannot be missed.
      unlisten = await api.onProgress(setProgress);
      if (!installedFiles) { await api.install(options); setInstalledFiles(true); }
      await api.applyOptions();
      setProgress(current => ({ ...current, phase: "complete", percent: 100 }));
      setPage("done");
    } catch (e) { setError(String(e)); }
    finally { unlisten?.(); setBusy(false); operation.current = false; }
  }
  async function launch() {
    if (operation.current) return;
    operation.current = true; setBusy(true); setError("");
    try { await api.launchApp(); }
    catch (e) { setError(String(e)); setBusy(false); operation.current = false; }
  }
  async function close() { try { await api.closeInstaller(); } catch (e) { setError(String(e)); } }
  const blocked = checking || !environment?.payload_available || environment.installed?.machine || environment.available_bytes < environment.required_bytes;
  const version = environment?.target_version ?? "0.0.8";

  return <div className="installer-window">
    <div className="ambient" aria-hidden="true">{[12, 29, 54, 77, 91].map((left, i) => <i key={left} style={{ left: `${left}%`, animationDelay: `${i * -3}s` }} />)}</div>
    <header data-tauri-drag-region className="titlebar"><span data-tauri-drag-region><b>DeepAgent Studio</b><span className="title-detail">安装程序 <span>·</span> v{version}</span></span><div className="window-controls"><Button size="icon" variant="ghost" aria-label="最小化" onClick={() => void api.minimize().catch(e => setError(String(e)))}><Minus size={15} /></Button><Button size="icon" variant="ghost" aria-label="关闭安装程序" disabled={busy} onClick={() => void close()}><X size={15} /></Button></div></header>

    <main className={`page page-${page}`} key={page}>
      {page === "welcome" && <>
        <div className="welcome-art"><div className="orbit" /><img className="brand-mark" src={brand} alt="" /><span className="art-dot" /></div>
        <div className="hero-copy"><span className="eyebrow">你的下一步，从这里开始</span><h1 ref={heading} tabIndex={-1}>DeepAgent Studio</h1><p>DeepSeek 原生的智能体工作台</p></div>
        <div className="welcome-action"><Button className="primary-cta" disabled={Boolean(blocked)} onClick={() => void begin()}>{checking ? "正在检查环境" : environment?.installed ? "升级安装" : "立即安装"}<ArrowRight size={16} /></Button><Button variant="ghost" className="custom-link" onClick={() => setPage("custom")}>自定义安装<ChevronRight size={14} /></Button></div>
        <div className="welcome-meta"><span><ShieldCheck size={14} />当前用户安装 · 无需管理员权限</span><span>Windows x64</span></div>
      </>}

      {page === "custom" && <>
        <div className="section-heading"><span className="eyebrow">安装偏好</span><h1 ref={heading} tabIndex={-1}>自定义安装</h1></div>
        <section className="location-section"><Label htmlFor="directory">安装位置</Label><div className="path-row"><div className="path-field"><FolderOpen size={16} /><Input id="directory" value={options.directory} onChange={e => { setOptions(current => ({ ...current, directory: e.target.value })); setChecking(true); }} onBlur={() => void inspect()} /></div><Button variant="outline" onClick={() => void browse()}>浏览</Button></div><div className="disk-info"><HardDrive size={13} />{checking ? "正在检查安装位置…" : environment?.payload_available ? `需要 ${bytes(environment.required_bytes)} · 目标磁盘可用 ${bytes(environment.available_bytes)}` : "开发预览：尚未嵌入安装包，未提供虚构容量"}</div></section>
        <div className="preference-grid"><section><h2>入口与快捷方式</h2><Option name="desktop_shortcut" title="创建桌面快捷方式" hint="从桌面快速打开工作台" options={options} setOptions={setOptions} /><Option name="start_menu_shortcut" title="添加到开始菜单" hint="通过 Windows 搜索启动" options={options} setOptions={setOptions} /></section><section><h2>命令行与启动</h2><Option name="add_to_path" title="将 deepagent 加入 PATH" hint="仅修改当前用户环境变量" options={options} setOptions={setOptions} /><Option name="autostart" title="登录 Windows 时启动" hint="默认关闭，启动正常工作台窗口" options={options} setOptions={setOptions} /></section></div>
        <div className="environment-note"><CheckCircle2 size={14} /><span>{environment?.webview_version ? `WebView2 ${environment.webview_version} 已就绪` : "WebView2 状态尚未确认"}</span><span>自动更新沿用主程序设置</span></div>
        <footer className="page-footer"><Button variant="ghost" onClick={() => setPage("welcome")}><ArrowLeft size={14} />返回</Button><Button className="footer-cta" disabled={Boolean(blocked)} onClick={() => void begin()}>开始安装<ArrowRight size={15} /></Button></footer>
      </>}

      {page === "installing" && <>
        <img className="install-brand" src={brand} alt="" /><span className="eyebrow">正在为你准备工作台</span><h1 ref={heading} tabIndex={-1}>{error ? "安装尚未完成" : phaseText[progress.phase]}</h1>
        <div className="progress-area"><div className="progress-top"><span>{progress.phase === "installing" ? "文件写入进度（估算）" : "安装进度"}</span><strong>{progress.percent}<small>%</small></strong></div><Progress value={progress.percent} aria-label="安装进度" /><div className="progress-caption"><span>{bytes(progress.written_bytes)} / {bytes(progress.total_bytes)}</span><span>完成后将校验文件完整性</span></div></div>
        <div className="install-note"><ShieldCheck size={17} /><p>由原生 NSIS 安装包完成安装，<br />已有的会话、设置和工作区数据不会在此步骤清理。</p></div>
        {!busy && error && <div className="retry-row"><Button onClick={() => void begin()}>{installedFiles ? "重试应用选项" : "重试安装"}</Button><Button variant="ghost" onClick={() => void close()}>关闭</Button></div>}
      </>}

      {page === "done" && <>
        <div className="success-symbol"><Check size={32} strokeWidth={1.7} /></div><span className="eyebrow">一切就绪</span><h1 ref={heading} tabIndex={-1}>安装完成</h1><p className="done-copy">打开 DeepAgent Studio，开始第一次协作。</p><Button className="primary-cta done-cta" onClick={() => void launch()} disabled={busy}>{busy ? "正在交接到工作台" : "立即体验"}<ArrowRight size={16} /></Button><Button variant="ghost" className="later-button" disabled={busy} onClick={() => void close()}>稍后打开</Button><div className="done-location"><FolderOpen size={13} /><span title={options.directory}>{options.directory}</span></div>
      </>}
    </main>
    {!busy && environment && !environment.payload_available && page !== "installing" && <div className="preview-note">开发预览 · 未嵌入 NSIS 包，安装按钮不可用</div>}
    {environment?.installed?.machine && <div className="message-strip" role="alert"><CircleAlert size={14} />检测到全用户安装，请使用原 NSIS 安装包维护。</div>}
    {environment && environment.available_bytes < environment.required_bytes && <div className="message-strip" role="alert"><CircleAlert size={14} />目标磁盘空间不足，请更换安装位置。</div>}
    {error && <div className="message-strip error-strip" role="alert"><CircleAlert size={14} /><span>{error}</span>{page !== "installing" && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void inspect()}>重新检查</Button>}</div>}
    {page === "welcome" && <div className="bottom-line"><span>Apache-2.0 开源软件</span><span>安装过程不连接模型服务</span></div>}
  </div>;
}
