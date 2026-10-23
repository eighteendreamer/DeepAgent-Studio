import { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { Check, Copy, Play } from "lucide-react";
import { Button } from "../../src/components/shadcn/button";
import { SendCard, type Handle, type SendVariant } from "./SendCard";
import type { MotionSettings } from "./sendMotion";

const settings: MotionSettings = { duration: .7, bounce: .55, arc: 22 };
const comparisons: { key: "original" | "refined"; variant: SendVariant }[] = [
  { key: "original", variant: { id: "F", name: "之前的 F", description: "保留原来的手感，放在这里方便比较", tag: "原版" } },
  { key: "refined", variant: { id: "F", name: "F · 一点 Q 弹", description: "轻轻蓄力、整块送入，像模型面板那样柔和回弹", tag: "本次调整" } },
];

function App() {
  const [slow, setSlow] = useState(false);
  const [selected, setSelected] = useState<"original" | "refined">("refined");
  const [copied, setCopied] = useState(false);
  const cards = useRef<Partial<Record<"original" | "refined", Handle | null>>>({});
  const copy = async () => {
    const text = selected === "refined" ? "选这版 F：整体弹送，带一点模型选择器展开时的 Q 弹。" : "保留之前的 F。";
    try { await navigator.clipboard.writeText(text); }
    catch {
      const input = document.createElement("textarea"); input.value = text;
      input.style.cssText = "position:fixed;opacity:0"; document.body.append(input);
      input.select(); document.execCommand("copy"); input.remove();
    }
    setCopied(true); window.setTimeout(() => setCopied(false), 1600);
  };
  return <main className="message-options-app refinement-app">
    <header className="options-header">
      <div><div className="page-eyebrow"><span/>DEEPAGENT · 发送动效微调</div>
        <h1>F，再加一点 Q 弹</h1><p>文字连着动作离开输入框，整块变成上方的消息气泡。</p>
      </div>
      <div className="header-controls">
        <Button variant="ghost" className={`slow-control ${slow ? "active" : ""}`} aria-pressed={slow} onClick={() => setSlow(value => !value)}>{slow ? "慢放中 ×0.55" : "慢放看细节"}</Button>
        <Button variant="outline" className="play-all" onClick={() => comparisons.forEach(item => cards.current[item.key]?.replay())}><Play size={12}/>一起播放对比</Button>
      </div>
    </header>
    <section className="refinement-grid" aria-label="F 原版与轻 Q 弹对比">
      {comparisons.map(item => <div key={item.key} data-comparison={item.key}>
        <SendCard ref={handle => { cards.current[item.key] = handle; }} variant={item.variant}
          motionProfile={item.key === "refined" ? "selector" : "classic"}
          selected={selected === item.key} onSelect={() => setSelected(item.key)} slow={slow} settings={settings}/>
      </div>)}
    </section>
    <div className="motion-notes"><span>轻压蓄力</span><i/><span>整体送入</span><i/><span>柔和回弹</span><small>右边是这次调整，点发送或重播体验</small></div>
    <footer className="selection-bar"><div><span className="selection-caption">当前选择</span><strong>{selected === "refined" ? "F · 一点 Q 弹" : "之前的 F"}</strong></div>
      <Button variant="secondary" className="copy-choice" onClick={copy}>{copied ? <Check size={13}/> : <Copy size={13}/>}<span>{copied ? "已复制" : "复制选择结果"}</span></Button>
    </footer>
    <p className="preview-note">预览第 2 版 · 可以改文字试发，也可以按 Enter 发送；Shift + Enter 换行。</p>
  </main>;
}
createRoot(document.getElementById("root")!).render(<App/>);
