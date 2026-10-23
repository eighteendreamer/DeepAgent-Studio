import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { gsap } from "gsap";
import { Book, Ellipsis, Layers, Search, SquarePen } from "lucide-react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faHouse } from "@fortawesome/free-solid-svg-icons";
import { Button } from "../../../apps/desktop/src/components/shadcn/button";
import { SlidingPill, useSlidingIndicator } from "../../../apps/desktop/src/components/ui/SlidingPill";
import { HoverInfo } from "../../../apps/desktop/src/components/ui/HoverInfo";

type Options = { duration: number; ease: string; exit: number; tip: number; instant?: boolean };
const items = [{ label: "新对话", Icon: SquarePen }, { label: "搜索", Icon: Search }, { label: "技能", Icon: Layers }, { label: "知识库", Icon: Book }, { label: "更多", Icon: Ellipsis }];
const presets = [
  { id: "A", name: "直接亮起", note: "指到即亮，取消滑行", options: { duration: 0, ease: "none", exit: 50, tip: 160, instant: true } },
  { id: "B", name: "利落短滑", note: "保留方向感，快起快停", options: { duration: 120, ease: "power2.out", exit: 60, tip: 160 } },
  { id: "C", name: "匀速平移", note: "匀速走完，不拖长尾", options: { duration: 160, ease: "none", exit: 60, tip: 160 } },
  { id: "D", name: "快速吸附", note: "迅速接近，短暂收住", options: { duration: 180, ease: "power4.out", exit: 60, tip: 160 } },
  { id: "E", name: "轻弹即停", note: "保留一点弹性，迅速结束", options: { duration: 220, ease: "back.out(0.7)", exit: 70, tip: 160 } },
] satisfies { id: string; name: string; note: string; options: Options }[];

function createMotion(marker: HTMLElement, tip: HTMLElement, rail: HTMLElement, options: () => Options) {
  let target: number | null = null, active: number | null = null;
  let move: gsap.core.Tween | undefined, tipTween: gsap.core.Tween | undefined, timer: gsap.core.Tween | undefined, demo: gsap.core.Timeline | undefined;
  const media = matchMedia("(prefers-reduced-motion: reduce)");
  const hideTip = () => { timer?.kill(); tipTween?.kill(); gsap.set(tip, { opacity: 0 }); tip.setAttribute("aria-hidden", "true"); };
  const show = (index: number | null, withTip = true) => {
    target = index; hideTip(); move?.kill();
    const config = options();
    marker.dataset.target = index == null ? "none" : String(index);
    if (index == null) { move = gsap.to(marker, { opacity: 0, duration: media.matches ? 0 : config.exit / 1000, overwrite: true }); return; }
    const y = index * 46;
    const visible = Number(gsap.getProperty(marker, "opacity")) > .01;
    if (!visible || config.instant || media.matches) {
      gsap.set(marker, { y, opacity: media.matches ? 1 : 0 });
      move = gsap.to(marker, { opacity: 1, duration: media.matches ? 0 : .055, overwrite: true });
    } else {
      move = gsap.to(marker, { y, opacity: 1, duration: config.duration / 1000, ease: config.ease, overwrite: true });
    }
    if (withTip) {
      timer = gsap.delayedCall(media.matches ? 0 : config.tip / 1000, () => {
        tip.textContent = items[index].label; gsap.set(tip, { top: y + 4 });
        tip.setAttribute("aria-hidden", "false");
        tipTween = gsap.to(tip, { opacity: 1, duration: media.matches ? 0 : .08, overwrite: true });
      });
    }
  };
  const stop = () => { demo?.kill(); demo = undefined; };
  const reduced = () => { stop(); move?.kill(); hideTip(); gsap.set(marker, { y: (target ?? 0) * 46, opacity: target == null ? 0 : 1 }); };
  media.addEventListener("change", reduced);
  return {
    enter(index: number, keyboard = false) { stop(); rail.dataset.pointed = String(index); show(index, true); if (keyboard) { timer?.kill(); tip.textContent = items[index].label; tip.setAttribute("aria-hidden", "false"); gsap.set(tip, { top: index * 46 + 4, opacity: 1 }); } },
    leave() { stop(); delete rail.dataset.pointed; show(active, false); },
    select(index: number) { stop(); active = index; rail.dataset.active = String(index); show(index, false); },
    run() {
      stop(); active = null; delete rail.dataset.active;
      demo = gsap.timeline();
      [0, 1, 2, 3, 4, 1, 4, 0].forEach((index, i) => demo!.call(() => { rail.dataset.pointed = String(index); show(index); }, [], i < 5 ? i * .23 : 1.1 + (i - 5) * .14));
      demo.call(() => { delete rail.dataset.pointed; show(null, false); }, [], 1.85);
    },
    dispose() { stop(); move?.kill(); hideTip(); media.removeEventListener("change", reduced); },
  };
}

function DemoFrame({ children, current = "新对话" }: { children: React.ReactNode; current?: string }) {
  return <div className="demo-window"><div className="demo-rail"><Button variant="ghost" size="icon" className="rail-home" aria-label="首页"><FontAwesomeIcon icon={faHouse} /></Button>{children}</div><div className="demo-pane"><strong>DeepAgent</strong><p className="pane-action">{current}</p><p className="pane-caption">项目</p><p className="pane-empty">暂无项目</p><p className="pane-caption">最近</p><p className="pane-empty">暂无对话</p><p className="pane-help">沿左侧图标上下移动<br />点击后可观察停靠</p></div></div>;
}

function Demo({ id, options, register }: { id: string; options: Options; register: (id: string, run?: () => void) => void }) {
  const rail = useRef<HTMLDivElement>(null), marker = useRef<HTMLDivElement>(null), tip = useRef<HTMLDivElement>(null);
  const latest = useRef(options); latest.current = options;
  const motion = useRef<ReturnType<typeof createMotion> | null>(null);
  const [current, setCurrent] = useState("新对话");
  useLayoutEffect(() => {
    const controller = createMotion(marker.current!, tip.current!, rail.current!, () => latest.current);
    motion.current = controller; register(id, () => controller.run());
    return () => { controller.dispose(); motion.current = null; register(id); };
  }, []);
  return <DemoFrame current={current}><div className="rail-items" ref={rail} data-demo={id}
    onPointerLeave={() => motion.current?.leave()}
    onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget as Node)) motion.current?.leave(); }}>
    <div ref={marker} className="nav-marker" aria-hidden="true" />
    {items.map(({ label, Icon }, index) => <Button key={label} variant="ghost" size="icon" className="rail-button" data-index={index} aria-label={`${id} ${label}`}
      onPointerEnter={() => motion.current?.enter(index)}
      onFocus={event => { if (event.currentTarget.matches(":focus-visible")) motion.current?.enter(index, true); }}
      onClick={() => { setCurrent(label); motion.current?.select(index); }}><Icon size={18} /></Button>)}
    <div ref={tip} className="rail-tip" aria-hidden="true" />
  </div></DemoFrame>;
}

function CurrentDemo() {
  const { containerRef, containerProps, indicatorStyle } = useSlidingIndicator({ hoverSelector: "[data-nav]", activeSelector: '[data-nav="start"]' });
  return <DemoFrame><div ref={containerRef} {...containerProps} className="rail-items current-demo" data-demo="current">
    {items.map(({ label, Icon }, index) => <HoverInfo key={label} content={label} side="right"><Button variant="ghost" size="icon" className="rail-button relative z-[1]" data-nav={index} aria-label={`当前 ${label}`}><Icon size={18} /></Button></HoverInfo>)}
    <SlidingPill style={indicatorStyle} className="current-marker left-0 right-0 rounded-[10px] bg-sidebar-highlight" />
  </div></DemoFrame>;
}

function App() {
  const [selected, setSelected] = useState<string | null>(null), [dark, setDark] = useState(false), [copied, setCopied] = useState(false);
  const [duration, setDuration] = useState(140), [ease, setEase] = useState("power2.out"), [exit, setExit] = useState(60), [tip, setTip] = useState(160);
  const demos = useRef(new Map<string, () => void>());
  const register = (id: string, run?: () => void) => { if (run) demos.current.set(id, run); else demos.current.delete(id); };
  const diy = { duration, ease, exit, tip };
  const choices = [...presets, { id: "F", name: "自己调手感", note: "调整速度、收尾和提示延迟", options: diy }];
  const chosen = choices.find(choice => choice.id === selected);
  const result = chosen ? `我选择方案 ${chosen.id}：${chosen.name}。移动 ${chosen.options.duration}ms，缓动 ${chosen.options.ease}，移出收尾 ${chosen.options.exit}ms，提示延迟 ${chosen.options.tip}ms。采用单块高亮，图标不随悬停移动。` : "";
  useEffect(() => { document.documentElement.classList.toggle("dark", dark); }, [dark]);
  useEffect(() => { setCopied(false); }, [result]);
  const copy = async () => {
    try { await navigator.clipboard.writeText(result); setCopied(true); }
    catch { const field = document.querySelector<HTMLTextAreaElement>("#selection-result")!; field.focus(); field.select(); setCopied(document.execCommand("copy")); }
  };
  return <main className="nav-review"><header><p className="eyebrow">DEEPAGENT STUDIO · 导航动效</p><h1>选一种更跟手的滑动</h1><p className="intro">在每个左侧图标栏上下扫动，比较高亮是否跟手、折返是否利落、移出是否干净</p><div className="toolbar"><Button variant="outline" onClick={() => demos.current.forEach(run => run())}>同时试跑六种方案</Button><Button variant="ghost" aria-pressed={dark} onClick={() => setDark(!dark)}>{dark ? "浅色预览" : "深色预览"}</Button><span>图标保持静止，只比较高亮与提示</span></div></header>
    <section className="choices" aria-label="六种导航动效方案">{choices.map(choice => <article className="motion-card" data-choice={choice.id} data-selected={selected === choice.id || undefined} key={choice.id}>
      <div className="card-title"><h2>{choice.id} · {choice.name}</h2><span>{choice.options.duration === 0 ? "无滑行" : `${choice.options.duration} ms`}</span></div><p className="card-note">{choice.note}</p>
      <Demo id={choice.id} options={choice.options} register={register} />
      {choice.id === "F" && <div className="diy-controls"><label>移动时长 <output>{duration} ms</output><input aria-label="DIY 移动时长" type="range" min="60" max="300" step="10" value={duration} onChange={e => setDuration(Number(e.target.value))} /></label><label>收尾时长 <output>{exit} ms</output><input aria-label="DIY 收尾时长" type="range" min="0" max="180" step="10" value={exit} onChange={e => setExit(Number(e.target.value))} /></label><label>提示延迟 <output>{tip} ms</output><input aria-label="DIY 提示延迟" type="range" min="0" max="400" step="20" value={tip} onChange={e => setTip(Number(e.target.value))} /></label><label>滑动方式<select aria-label="DIY 滑动方式" value={ease} onChange={e => setEase(e.target.value)}><option value="power2.out">利落减速</option><option value="none">匀速移动</option><option value="power4.out">快速吸附</option><option value="back.out(0.7)">轻弹一下</option></select></label></div>}
      <div className="card-actions"><Button variant="ghost" onClick={() => demos.current.get(choice.id)?.()}>试跑</Button><Button variant="outline" aria-pressed={selected === choice.id} onClick={() => setSelected(choice.id)}>{selected === choice.id ? `已选 ${choice.id}` : `选择 ${choice.id}`}</Button></div>
    </article>)}</section>
    <details className="baseline"><summary>查看当前代码的效果对照</summary><div className="baseline-body"><CurrentDemo /><p>这里复用当前高亮组件的 300 ms 过渡，并保留按钮自身的悬停底色与现有提示组件。<br />六个新方案都只使用一块高亮，移到新按钮时会打断上一段滑动。</p></div></details>
    <section className="selection" aria-label="选择结果"><div><strong>{chosen ? `已选 ${chosen.id} · ${chosen.name}` : "选择你喜欢的手感"}</strong><p>也可以直接在聊天里回复字母</p></div><Button disabled={!chosen} onClick={copy}>{copied ? "已复制选择" : "复制选择结果"}</Button><textarea id="selection-result" aria-label="选择结果文本" readOnly value={result} /></section>
    <footer>基于项目现有导航尺寸与图标制作 · 动效实现参考 <a href="https://gsap.com/docs/v3/GSAP/gsap.to%28%29/" target="_blank" rel="noreferrer">GSAP 官方文档</a></footer>
  </main>;
}
createRoot(document.getElementById("root")!).render(<React.StrictMode><App /></React.StrictMode>);
