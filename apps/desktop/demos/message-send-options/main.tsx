import { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { Check, Copy, Play, SlidersHorizontal } from "lucide-react";
import { Button } from "../../src/components/shadcn/button";
import { Slider } from "../../src/components/shadcn/slider";
import { type MotionSettings, type Variant } from "./sendMotion";
import { SendCard, type Handle } from "./SendCard";

const variants: { id: Variant; name: string; description: string; tag: string }[] = [
  { id: "A", name: "原位上送", description: "文字原地抽离，完整接入上方气泡", tag: "轻 · 连贯" },
  { id: "B", name: "软胶黏连", description: "短暂拉出一段软胶，再轻轻断开", tag: "软 · 黏连" },
  { id: "C", name: "逐字接力", description: "同一批文字逐个离开输入框，再排成气泡", tag: "细 · 文字" },
  { id: "D", name: "揭片入泡", description: "像揭下一张便签，带着文字轻轻落位", tag: "轻 · 立体" },
  { id: "E", name: "轨迹牵引", description: "沿着一条短弧线，将输入和气泡连起来", tag: "顺 · 轨迹" },
  { id: "F", name: "压缩弹送", description: "先收紧、再弹出，落位后轻轻回弹", tag: "弹 · 可调" },
];
function App() {
  const [selected, setSelected] = useState<Variant>("A");
  const [slow, setSlow] = useState(false);
  const [settings, setSettings] = useState<MotionSettings>({ duration: .7, bounce: .55, arc: 22 });
  const [copied, setCopied] = useState(false);
  const cards = useRef<Partial<Record<Variant, Handle | null>>>({});
  const option = variants.find(item => item.id === selected)!;
  const choice = `我选择方案 ${selected}：${option.name}。${selected === "F" ? `时长 ${settings.duration.toFixed(2)} 秒，回弹 ${Math.round(settings.bounce * 100)}%，弧度 ${settings.arc}px。` : ""}`;
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(choice);
      setCopied(true); window.setTimeout(() => setCopied(false), 1600);
    } catch {
      const input = document.createElement("textarea"); input.value = choice;
      input.style.cssText = "position:fixed;opacity:0"; document.body.append(input);
      input.select(); document.execCommand("copy"); input.remove(); setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    }
  };
  return <div className="message-options-app">
    <header className="options-header">
      <div><div className="page-eyebrow"><span/>DEEPAGENT · 交互动效选择</div><h1>从输入框，到消息气泡</h1><p>同一句话，六种送达的方式。点箭头发送，也可以一起播放比较。</p></div>
      <div className="header-controls"><Button variant="ghost" className={`slow-control ${slow ? "active" : ""}`} aria-pressed={slow} onClick={() => setSlow(value => !value)}>{slow ? "慢放中 ×0.55" : "慢放看细节"}</Button>
        <Button variant="outline" className="play-all" onClick={() => variants.forEach(item => cards.current[item.id]?.replay())}><Play size={12}/>一起播放</Button></div>
    </header>
    <section className="send-options-grid" aria-label="六种发送动画方案">{variants.map(variant => <SendCard key={variant.id} ref={handle => { cards.current[variant.id] = handle; }} variant={variant} selected={selected === variant.id} onSelect={() => setSelected(variant.id)} slow={slow} settings={settings}/>)}</section>
    <section className="motion-settings" aria-label="方案 F 自定义参数">
      <div className="settings-label"><SlidersHorizontal size={14}/><span>F · 调一下手感</span><small>改完点 F 的重播按钮</small></div>
      <div className="setting-controls">{[
        { key: "duration" as const, label: "发送时长", min: .4, max: 1.2, step: .05, value: `${settings.duration.toFixed(2)}s` },
        { key: "bounce" as const, label: "回弹力度", min: 0, max: 1, step: .05, value: `${Math.round(settings.bounce*100)}%` },
        { key: "arc" as const, label: "弧线幅度", min: -50, max: 60, step: 5, value: `${settings.arc}px` },
      ].map(control => <div className="setting-field" key={control.key}><label id={`setting-${control.key}`}>{control.label}<span>{control.value}</span></label><Slider className="diy-slider" thumbProps={{ "aria-labelledby": `setting-${control.key}` }} min={control.min} max={control.max} step={control.step} value={[settings[control.key]]} onValueChange={value => setSettings(previous => ({ ...previous, [control.key]: value[0] }))}/></div>)}</div>
    </section>
    <footer className="selection-bar"><div><span className="selection-caption">当前选择</span><strong>{selected} · {option.name}</strong><span className="selection-result" data-choice>{choice}</span></div>
      <Button variant="secondary" className="copy-choice" onClick={copy}>{copied ? <Check size={13}/> : <Copy size={13}/>}<span>{copied ? "已复制" : "复制选择结果"}</span></Button>
    </footer>
    <p className="preview-note">可以直接回复 A–F。这里仅预览发送动作，选定后再接入项目。</p>
  </div>;
}
createRoot(document.getElementById("root")!).render(<App/>);
