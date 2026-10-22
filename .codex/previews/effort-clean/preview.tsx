import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { Brain } from "lucide-react";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { ModelThinkingSelector } from "../../../apps/desktop/src/components/ModelThinkingSelector";

i18n.use(initReactI18next).init({ lng: "zh", resources: { zh: { translation: {} } }, interpolation: { escapeValue: false } });
const events: string[] = [];
const options = [{ id: "simple", label: "简单", icon: Brain }, { id: "medium", label: "中度", icon: Brain }, { id: "deep", label: "深度", icon: Brain }] as const;

function Preview() {
  const [value, setValue] = useState<"simple" | "medium" | "deep">("deep");
  const [model, setModel] = useState("deepseek-flash");
  const [open, setOpen] = useState(true);
  const [dark, setDark] = useState(false);
  const [disabled, setDisabled] = useState(false);
  Object.assign(window, { effortReview: { events }, setReviewValue: setValue, setReviewOpen: setOpen, setReviewDisabled: setDisabled });
  return <main className={dark ? "effort-review dark" : "effort-review"}>
    <header className="review-header"><p>DEEPAGENT STUDIO · A</p><h1>果冻圆珠 · 精简版</h1></header>
    <section className="review-stage" aria-label="模型与推理强度弹窗预览">
      <div className="review-anchor">
        <ModelThinkingSelector open={open} disabled={disabled} models={["deepseek-flash", "deepseek-v4-pro"]}
          selectedModel={model} selectedThinking={value} thinkingOptions={options}
          selectModelLabel="选择模型" noModelsLabel="暂无模型" onOpenChange={setOpen}
          onChooseModel={next => { events.push(`model:${next}`); setModel(next); }}
          onChooseThinking={next => { events.push(next); setValue(next); }} />
      </div>
    </section>
    <div className="review-actions"><button type="button" onClick={() => { setValue("deep"); setModel("deepseek-flash"); setOpen(true); }}>重置预览</button><button type="button" onClick={() => { setDark(!dark); setOpen(true); }}>{dark ? "浅色预览" : "深色预览"}</button></div>
    <p className="review-note">直接拖动圆珠，或点击三档标签。此页使用桌面端的实际弹窗与动效组件。</p>
    <output className="sr-only" aria-label="当前强度">{value}</output>
  </main>;
}
createRoot(document.getElementById("root")!).render(<React.StrictMode><Preview /></React.StrictMode>);
