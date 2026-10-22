import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { gsap } from "gsap";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { EffortSlider } from "../../../apps/desktop/src/components/ui/EffortSlider";

i18n.use(initReactI18next).init({ lng: "zh", resources: { zh: { translation: {} } }, interpolation: { escapeValue: false } });
const events: string[] = [];
Object.assign(window, { effortReview: { events } });
function Preview() {
  const [value, setValue] = useState("medium");
  const [dark, setDark] = useState(false);
  const [slow, setSlow] = useState(false);
  const [disabled, setDisabled] = useState(false);
  const [mounted, setMounted] = useState(true);
  Object.assign(window, { setReviewValue: setValue, setReviewDisabled: setDisabled, setReviewMounted: setMounted });
  return <main className={dark ? "review dark" : "review"}>
    <header><p className="eyebrow">DEEPAGENT STUDIO · A</p><h1>果冻圆珠</h1><p className="intro">前端跟手，尾部拖后。快速反向时甩尾，松手后轻轻弹回圆形。</p></header>
    <section className="stage" aria-label="A 方案果冻圆珠预览">
      <div className="model-panel"><div className="model-row"><span>模型</span><span>deepseek-v4-pro</span></div><div className="divider"/><h2>推理强度</h2>
        {mounted && <EffortSlider stops={[{ value: "simple", label: "简单" }, { value: "medium", label: "中度" }, { value: "deep", label: "深度" }]}
          value={value} onChange={next => { events.push(next); setValue(next); }} ariaLabel="果冻圆珠思考强度" disabled={disabled}/>}
      </div>
    </section>
    <p className="hint">按住圆珠左右拖动；快速折返，更容易看到尾部的惯性。</p>
    <div className="actions"><button type="button" aria-pressed={slow} onClick={() => { gsap.globalTimeline.timeScale(slow ? 1 : 0.45); setSlow(!slow); }}>慢速观察</button><button type="button" aria-pressed={dark} onClick={() => setDark(!dark)}>{dark ? "浅色预览" : "深色预览"}</button><button type="button" onClick={() => setValue("medium")}>重置到中度</button></div>
    <div className="notes"><span>静止接近正圆</span><span>拖动拉伸与甩尾</span><span>松手吸附与回弹</span></div>
    <p className="footnote">沿用 A 的 26 px 圆珠与平滑点击过渡。此预览和桌面端使用同一组件与动效代码。</p>
    <output aria-label="当前强度" className="sr-only">{value}</output>
  </main>;
}
createRoot(document.getElementById("root")!).render(<React.StrictMode><Preview/></React.StrictMode>);
