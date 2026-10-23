import React, { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import gsap from "gsap";
import {
  ArrowUp, ArrowUpRight, BookOpen, Check, ChevronDown, ChevronRight,
  Command, Copy, Ellipsis, Folder, FolderOpen, Layers, Menu, MessageSquare,
  Monitor, Moon, PanelLeft, Pin, Plus, Search, Settings, SlidersHorizontal,
  Sparkles, SquarePen, Sun, X,
} from "lucide-react";
import { Button } from "../../src/components/shadcn/button";
import { Input } from "../../src/components/shadcn/input";
import { Textarea } from "../../src/components/shadcn/textarea";
import { Slider } from "../../src/components/shadcn/slider";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../../src/components/shadcn/dropdown-menu";
import { SpringDisclosure } from "../../src/components/ui/SpringDisclosure";
import { prefersReducedMotion } from "../../src/components/ui/springMotion";
import "../../src/styles.css";
import "./preview.css";

type OptionId = "A" | "B" | "C" | "D";
type SettingsValue = { width: number; radius: number; gap: number };
const options: { id: OptionId; name: string; summary: string; detail: string; defaults: SettingsValue }[] = [
  { id: "A", name: "贴边内嵌", summary: "窄留缝 · 单一底层", detail: "最接近参考站。侧栏融入底层，对话区内嵌，左上角明确分开两个区域。", defaults: { width: 248, radius: 24, gap: 8 } },
  { id: "B", name: "悬浮画布", summary: "宽留缝 · 更多呼吸感", detail: "让对话区从侧栏和窗口边缘退开一点，用留白与轻阴影区分层级。", defaults: { width: 236, radius: 28, gap: 16 } },
  { id: "C", name: "双层卡片", summary: "双面板 · 对齐的边界", detail: "侧栏和对话各自成为独立面板，四周边界完整，适合清晰的工作台布局。", defaults: { width: 248, radius: 20, gap: 14 } },
  { id: "D", name: "双栏导航", summary: "功能栏 + 会话栏", detail: "常用功能独立成窄栏，项目和会话集中在第二栏，减少不同信息之间的混排。", defaults: { width: 232, radius: 24, gap: 8 } },
];
const initialSettings = Object.fromEntries(options.map(option => [option.id, { ...option.defaults }])) as Record<OptionId, SettingsValue>;
const initialThreads = [
  { id: "corners", title: "圆角与侧栏设计", group: "today" },
  { id: "composer", title: "输入区的交互细节", group: "today" },
  { id: "runtime", title: "梳理 Agent 运行流程", group: "today" },
  { id: "theme", title: "工作台的深色主题", group: "week" },
  { id: "docs", title: "整理项目开发文档", group: "week" },
];
const palettes = {
  dark: { background: "#18181a", canvas: "#0d0d0f", side: "#1b1b1e", fg: "#eeeeef", muted: "#a2a2ab", line: "#303034", hover: "#28282d", active: "#303035", user: "#252528", field: "#1b1b1e", focus: "#a6b6d0" },
  light: { background: "#efefec", canvas: "#ffffff", side: "#f7f7f4", fg: "#292b30", muted: "#656870", line: "#deded9", hover: "#e8e8e3", active: "#e0e1db", user: "#f0f0ec", field: "#f7f7f5", focus: "#3b65a0" },
};
const initialUser = "参考这种侧栏和对话区的分隔方式，帮我调整一下 DeepAgent 的布局。";

function MiniLayout({ id }: { id: OptionId }) {
  return <span className={`sb-mini sb-mini-${id}`} aria-hidden="true"><i /><b /><em /></span>;
}

function App() {
  const [id, setId] = useState<OptionId>("D");
  const [settings, setSettings] = useState(initialSettings);
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const [adjusting, setAdjusting] = useState(false);
  const [sidebar, setSidebar] = useState(true);
  const [mobileNav, setMobileNav] = useState(false);
  const [projectsOpen, setProjectsOpen] = useState(true);
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [active, setActive] = useState("corners");
  const [threads, setThreads] = useState(initialThreads);
  const [empty, setEmpty] = useState(false);
  const [prompt, setPrompt] = useState("");
  const [localMessages, setLocalMessages] = useState<string[]>([]);
  const [copied, setCopied] = useState(false);
  const [notice, setNotice] = useState("");
  const [details, setDetails] = useState(false);
  const stage = useRef<HTMLDivElement>(null);
  const messagesEnd = useRef<HTMLDivElement>(null);
  const copyField = useRef<HTMLTextAreaElement>(null);
  const searchField = useRef<HTMLInputElement>(null);
  const option = options.find(item => item.id === id)!;
  const value = settings[id];
  const palette = palettes[theme];
  const current = threads.find(thread => thread.id === active);
  const selectedText = `我选择方案 ${id}：${option.name}。侧栏宽度 ${value.width}px，面板圆角 ${value.radius}px，边缘留缝 ${value.gap}px，连续圆角平滑度 60%。${theme === "dark" ? "深色" : "浅色"}预览。请按这一布局调整 DeepAgent 的侧栏与对话区，保留现有功能。`;
  const variables = {
    "--sb-background": palette.background, "--sb-canvas": palette.canvas, "--sb-side": palette.side,
    "--sb-fg": palette.fg, "--sb-muted": palette.muted, "--sb-line": palette.line,
    "--sb-hover": palette.hover, "--sb-active": palette.active, "--sb-user": palette.user,
    "--sb-field": palette.field, "--sb-focus": palette.focus,
    "--theme-bg": palette.canvas, "--theme-sidebar": palette.side, "--theme-fg": palette.fg,
    "--theme-text-secondary": palette.muted, "--theme-border": palette.line,
    "--theme-elevated": palette.side, "--theme-hover": palette.hover,
    "--theme-accent": palette.fg, "--theme-accent-hover": palette.fg,
    "--sb-width": `${value.width}px`, "--sb-radius": `${value.radius}px`, "--sb-gap": `${value.gap}px`,
  } as CSSProperties;

  useLayoutEffect(() => {
    if (!stage.current || prefersReducedMotion()) return;
    const context = gsap.context(() => {
      gsap.fromTo(".sb-canvas", { opacity: .65 }, { opacity: 1, duration: .22, ease: "power2.out" });
      gsap.fromTo(".sb-thread-list", { opacity: .55 }, { opacity: 1, duration: .24, ease: "power2.out" });
    }, stage);
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const settle = () => { if (media.matches) context.revert(); };
    media.addEventListener("change", settle);
    return () => { context.revert(); media.removeEventListener("change", settle); };
  }, [id]);

  useEffect(() => { setCopied(false); }, [selectedText]);
  useEffect(() => { if (searching) searchField.current?.focus(); }, [searching]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => { if (event.key === "Escape") { setMobileNav(false); setSearching(false); } };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(""), 2800);
    return () => window.clearTimeout(timer);
  }, [notice]);

  function choose(next: OptionId) { setId(next); setSidebar(true); setMobileNav(false); }
  function update(key: keyof SettingsValue, next: number) { setSettings(previous => ({ ...previous, [id]: { ...previous[id], [key]: next } })); }
  function newChat() { setEmpty(true); setActive(""); setLocalMessages([]); setPrompt(""); setMobileNav(false); }
  function openThread(next: string) { setActive(next); setEmpty(false); setLocalMessages([]); setMobileNav(false); }
  function submit(event: FormEvent) {
    event.preventDefault();
    if (!prompt.trim()) return;
    setLocalMessages(previous => [...previous, prompt.trim()]); setPrompt(""); setEmpty(false);
    setNotice("内容已显示在预览中");
    requestAnimationFrame(() => messagesEnd.current?.scrollIntoView({ block: "nearest", behavior: "instant" }));
  }
  async function copyChoice() {
    setDetails(true);
    try {
      if (!navigator.clipboard?.writeText) throw new Error("Clipboard unavailable");
      await navigator.clipboard.writeText(selectedText); setCopied(true);
    } catch {
      setNotice("请复制下方的方案文字");
      requestAnimationFrame(() => { copyField.current?.focus(); copyField.current?.select(); });
    }
  }
  const visibleThreads = threads.filter(thread => thread.title.toLowerCase().includes(query.toLowerCase()));
  const navigation = [
    { icon: SquarePen, label: "新对话", action: newChat },
    { icon: Search, label: "搜索", action: () => { setSearching(previous => !previous); setSidebar(true); setMobileNav(true); } },
    { icon: Layers, label: "技能", action: () => setNotice("技能入口的外观示意") },
    { icon: BookOpen, label: "知识库", action: () => setNotice("知识库入口的外观示意") },
  ];

  return <div className="sb-choice-page">
    <div className="sb-page-inner">
      <header className="sb-page-header">
        <div><p className="sb-eyebrow"><span /> DEEPAGENT STUDIO <span className="sb-eyebrow-divider">/</span> 布局探索</p>
          <h1>侧栏与对话区的四种布局</h1>
          <p className="sb-intro">已确认 D 双栏导航，保留苹果连续圆角，移除对话标题下方横线</p>
        </div>
        <a href="https://hextaui.com/blocks/chat-sidebar" target="_blank" rel="noreferrer" className="sb-reference-link">查看参考站<ArrowUpRight size={14} /></a>
      </header>

      <div className="sb-option-grid" role="group" aria-label="布局方案">
        {options.map(item => <Button key={item.id} variant="ghost" className="sb-option-card" aria-pressed={id === item.id} onClick={() => choose(item.id)} data-option={item.id}>
          <MiniLayout id={item.id} /><span className="sb-option-copy"><span className="sb-option-name"><small>{item.id}</small><strong>{item.name}</strong>{item.id === "A" && <em>接近参考</em>}</span><span className="sb-option-summary">{item.summary}</span></span>
          <span className="sb-card-check">{id === item.id && <Check size={12} />}</span>
        </Button>)}
      </div>

      <div className="sb-stage-toolbar">
        <div className="sb-toolbar-description"><span className="sb-stage-number">{id}</span><span>{option.name}</span><i /><span className="sb-toolbar-note">可以操作侧栏和输入框</span></div>
        <div className="sb-toolbar-controls">
          <div className="sb-theme-switch" role="group" aria-label="预览主题">
            <Button variant="ghost" aria-label="浅色预览" aria-pressed={theme === "light"} onClick={() => setTheme("light")}><Sun size={14} /></Button>
            <Button variant="ghost" aria-label="深色预览" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}><Moon size={14} /></Button>
          </div>
          <Button variant="ghost" className="sb-adjust-trigger" aria-expanded={adjusting} onClick={() => setAdjusting(previous => !previous)}><SlidersHorizontal size={13} /><span>微调</span></Button>
        </div>
      </div>

      <SpringDisclosure open={adjusting} className="sb-adjust-disclosure">
        <div className="sb-adjustments">
          {([{ key: "width", label: "侧栏宽度", min: 216, max: 288 }, { key: "radius", label: "面板圆角", min: 16, max: 32 }, { key: "gap", label: "边缘留缝", min: 4, max: 24 }] as const).map(control => <div key={control.key}>
            <label id={`sb-${control.key}`}>{control.label}<output>{value[control.key]}px</output></label>
            <Slider className="sb-parameter-slider" value={[value[control.key]]} min={control.min} max={control.max} step={1} onValueChange={([next]) => update(control.key, next)} thumbProps={{ "aria-labelledby": `sb-${control.key}` }} />
          </div>)}
          <Button variant="ghost" className="sb-reset" onClick={() => setSettings(previous => ({ ...previous, [id]: { ...option.defaults } }))}>恢复默认</Button>
        </div>
      </SpringDisclosure>

      <div ref={stage} className={`sb-stage ${theme === "dark" ? "dark" : ""}`} data-theme={theme} data-layout={id} data-sidebar={sidebar} data-mobile-nav={mobileNav} style={variables}>
        <div className="sb-frame">
          {id === "D" && <nav className="sb-rail" aria-label="功能栏"><span className="sb-mark"><Command size={19} /></span>
            {navigation.map(item => <Button key={item.label} variant="ghost" className="sb-rail-button" aria-label={item.label} onClick={item.action}><item.icon size={18} /></Button>)}
            <span className="sb-rail-spacer" /><Button variant="ghost" className="sb-rail-button" aria-label="外观设置" onClick={() => setTheme(theme === "dark" ? "light" : "dark")}><Settings size={18} /></Button><span className="sb-avatar">雷</span>
          </nav>}

          <aside className="sb-sidebar" aria-label="聊天侧栏" aria-hidden={!sidebar && !mobileNav}>
            <div className="sb-sidebar-top"><span className="sb-sidebar-logo"><span className="sb-mark"><Command size={18} /></span><strong>{id === "D" ? "项目与对话" : "DeepAgent"}</strong></span>
              <Button variant="ghost" className="sb-icon sb-collapse-desktop" aria-label="收起侧栏" onClick={() => setSidebar(false)}><PanelLeft size={16} /></Button>
              <Button variant="ghost" className="sb-icon sb-collapse-mobile" aria-label="关闭侧栏" onClick={() => setMobileNav(false)}><X size={16} /></Button>
            </div>

            {id !== "D" && <nav className="sb-nav" aria-label="主要功能">{navigation.map(item => <Button key={item.label} variant="ghost" className={`sb-nav-row ${item.label === "新对话" ? "sb-nav-new" : ""}`} onClick={item.action}><item.icon size={16} /><span>{item.label}</span>{item.label === "新对话" && <kbd>Ctrl O</kbd>}</Button>)}</nav>}
            {id === "D" && <Button variant="ghost" className="sb-nav-row sb-nav-new" onClick={newChat}><Plus size={16} /><span>新对话</span></Button>}
            <SpringDisclosure open={searching}><div className="sb-search"><Search size={14} /><Input ref={searchField} aria-label="搜索会话" placeholder="搜索会话..." value={query} onChange={event => setQuery(event.target.value)} /><Button variant="ghost" className="sb-icon" aria-label="清空搜索" onClick={() => { setQuery(""); setSearching(false); }}><X size={13} /></Button></div></SpringDisclosure>

            <div className="sb-sidebar-scroll">
              <div className="sb-section-label"><span>项目</span><Button variant="ghost" className="sb-icon" aria-label="添加项目示意" onClick={() => setNotice("添加项目入口的外观示意")}><Plus size={13} /></Button></div>
              <Button variant="ghost" className="sb-project-row" aria-expanded={projectsOpen} onClick={() => setProjectsOpen(previous => !previous)}><FolderOpen size={15} /><span>DeepAgent Studio</span>{projectsOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</Button>
              <SpringDisclosure open={projectsOpen}><div className="sb-project-children"><Button variant="ghost" className={active === "corners" ? "sb-project-chat is-active" : "sb-project-chat"} onClick={() => openThread("corners")}><span className="sb-project-dot" />界面设计与交互</Button><Button variant="ghost" className="sb-project-chat" onClick={() => openThread("runtime")}><span className="sb-project-dot" />运行时与工具</Button></div></SpringDisclosure>
              <Button variant="ghost" className="sb-project-row" onClick={() => setNotice("设计资料项目的外观示意")}><Folder size={15} /><span>设计资料</span><ChevronRight size={13} /></Button>

              <div className="sb-thread-list">
                {([{ key: "today", label: "今天" }, { key: "week", label: "最近 7 天" }] as const).map(group => <section key={group.key} className="sb-thread-group"><div className="sb-section-label"><span>{group.label}</span></div>
                  {visibleThreads.filter(thread => thread.group === group.key).map(thread => <div key={thread.id} className={`sb-chat-row ${active === thread.id ? "is-active" : ""}`}>
                    <Button variant="ghost" className="sb-thread-button" onClick={() => openThread(thread.id)}><span>{thread.title}</span></Button>
                    <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" className="sb-icon sb-thread-menu" aria-label={`${thread.title}的菜单`}><Ellipsis size={14} /></Button></DropdownMenuTrigger>
                      <DropdownMenuContent className="sb-popup" align="start" style={variables}>
                        <DropdownMenuItem onSelect={() => setNotice(`已选择「${thread.title}」的置顶示意`)}><Pin size={14} />置顶会话</DropdownMenuItem>
                        <DropdownMenuItem onSelect={() => setNotice(`重命名「${thread.title}」的入口示意`)}><SquarePen size={14} />重命名</DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem onSelect={() => { setThreads(previous => previous.filter(item => item.id !== thread.id)); if (active === thread.id) newChat(); }}>从预览移除</DropdownMenuItem>
                      </DropdownMenuContent></DropdownMenu>
                  </div>)}
                </section>)}
                {visibleThreads.length === 0 && <p className="sb-no-results">没有找到会话</p>}
              </div>
            </div>

            <div className="sb-sidebar-bottom"><span className="sb-avatar">雷</span><span className="sb-account"><strong>我的工作空间</strong><small>本地模式</small></span>
              <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" className="sb-icon" aria-label="工作空间设置"><Settings size={16} /></Button></DropdownMenuTrigger><DropdownMenuContent className="sb-popup" side="top" align="start" style={variables}><DropdownMenuItem onSelect={() => setTheme("light")}><Sun size={14} />浅色外观</DropdownMenuItem><DropdownMenuItem onSelect={() => setTheme("dark")}><Moon size={14} />深色外观</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem onSelect={() => setSidebar(false)}><PanelLeft size={14} />收起侧栏</DropdownMenuItem></DropdownMenuContent></DropdownMenu>
            </div>
          </aside>

          {mobileNav && <Button variant="ghost" className="sb-mobile-scrim" aria-label="关闭侧栏遮罩" onClick={() => setMobileNav(false)} />}

          <main className="sb-canvas" aria-label="对话区">
            <header className="sb-chat-header">
              <Button variant="ghost" className="sb-icon sb-expand-desktop" aria-label="展开侧栏" onClick={() => setSidebar(true)}><PanelLeft size={17} /></Button>
              <Button variant="ghost" className="sb-icon sb-open-mobile" aria-label="打开侧栏" onClick={() => setMobileNav(true)}><Menu size={17} /></Button>
              <div className="sb-chat-heading"><span className="sb-breadcrumb">DeepAgent Studio<ChevronRight size={11} /></span><strong>{empty ? "新对话" : current?.title ?? "新对话"}</strong></div>
              <div className="sb-chat-top-actions"><span className="sb-local-badge"><span />本地</span><Button variant="ghost" className="sb-icon" aria-label="重置示例对话" onClick={() => { setThreads(initialThreads); openThread("corners"); }}><MessageSquare size={16} /></Button><Button variant="ghost" className="sb-icon" aria-label="侧栏方案说明" onClick={() => setNotice(option.detail)}><Ellipsis size={17} /></Button></div>
            </header>

            <div className={`sb-conversation ${empty ? "is-empty" : ""}`}>
              {empty ? <div className="sb-empty"><span className="sb-empty-mark"><Command size={27} /></span><h2>我们应该做些什么？</h2><p>从一个想法开始，或继续左侧的会话</p></div> : <div className="sb-messages">
                <div className="sb-message-user">{active === "corners" ? initialUser : `继续讨论${current?.title ?? "这个任务"}，先看看这个布局里的对话效果。`}</div>
                <div className="sb-message-assistant"><div className="sb-assistant-label"><span className="sb-small-mark"><Command size={14} /></span><strong>DeepAgent</strong><span>示例内容</span></div>
                  <p>可以。保留熟悉的工作方式，让侧栏与对话区的边界更清楚。</p>
                  <div className="sb-answer-summary"><span><Check size={13} />项目和最近会话集中在侧栏</span><span><Check size={13} />对话区用独立面板承载</span><span><Check size={13} />连续圆角统一整个界面</span></div>
                  <p>你可以切换上方四种方案，也可以试试收起侧栏、搜索会话和切换浅深色，比较实际的空间感。</p>
                  <div className="sb-answer-actions"><Button variant="ghost" className="sb-icon" aria-label="示例消息操作" onClick={() => setNotice("示例回复的操作入口")}><Copy size={14} /></Button><span>仅用于布局预览</span></div>
                </div>
                {localMessages.map((message, index) => <div className="sb-message-user" key={`${index}-${message}`}>{message}</div>)}
                <div ref={messagesEnd} />
              </div>}
            </div>

            <div className="sb-composer-area"><form className="sb-composer" onSubmit={submit}>
              <Textarea aria-label="预览消息输入" className="sb-message-input" value={prompt} onChange={event => setPrompt(event.target.value)} placeholder="尽管问..." onKeyDown={event => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (prompt.trim()) submit(event); } }} />
              <div className="sb-composer-tools"><Button variant="ghost" className="sb-icon" aria-label="添加附件示意" onClick={() => setNotice("附件入口的外观示意")}><Plus size={17} /></Button><span className="sb-approval-chip">请求批准<ChevronDown size={10} /></span><span className="sb-composer-spacer" /><span className="sb-model-chip"><Sparkles size={13} /><b>deepseek-flash</b><span>深度</span><ChevronDown size={10} /></span><Button type="submit" className="sb-send" aria-label="在预览中发送" disabled={!prompt.trim()}><ArrowUp size={16} /></Button></div>
            </form><div className="sb-composer-footer"><span><Folder size={12} />DeepAgent Studio<ChevronDown size={10} /></span><span><Monitor size={12} />本地模式</span><small>示例会话</small></div></div>
          </main>
        </div>
      </div>

      <div className="sb-selection-footer"><div><p><strong>{id} · {option.name}</strong><span>{option.detail}</span></p><div className="sb-spec-tags"><span>侧栏 {value.width}px{id === "D" ? " + 功能栏" : ""}</span><span>圆角 {value.radius}px</span><span>留缝 {value.gap}px</span><span>平滑度 60%</span></div></div><Button className="sb-copy-selection" onClick={copyChoice}>{copied ? <Check size={15} /> : <Copy size={14} />}{copied ? "已复制方案" : "复制选择结果"}</Button></div>
      <SpringDisclosure open={details}><Textarea ref={copyField} readOnly className="sb-selection-text" aria-label="方案选择结果" value={selectedText} /></SpringDisclosure>
      <footer className="sb-page-footer"><span>D 方案已确认 · 正式布局已同步调整</span><span>可继续切换对比，或微调圆角与留缝</span></footer>
    </div>
    {notice && <div className="sb-notice" role="status">{notice}</div>}
  </div>;
}

createRoot(document.getElementById("root")!).render(<App />);
