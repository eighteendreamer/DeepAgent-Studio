import React, { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import gsap from "gsap";
import {
  ArrowUp, Brain, ChevronDown, ChevronRight, Folder, Hand, Layers,
  Library, Monitor, MoreHorizontal, PanelLeft, Plus, Search, Settings, SquarePen, Wallet,
} from "lucide-react";
import { Button } from "../../src/components/shadcn/button";
import { Slider } from "../../src/components/shadcn/slider";
import { InputSurface } from "../../src/components/ui/InputSurface";
import { prefersReducedMotion } from "../../src/components/ui/springMotion";
import "../../src/styles.css";
import "./preview.css";
import { cornerPath } from "./curve";

function Surface({ radius, smoothing, className = "", children }: {
  radius: number; smoothing: number; className?: string; children: ReactNode;
}) {
  const host = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const node = host.current;
    if (!node) return;
    const observer = new ResizeObserver(() => setSize({
      width: node.clientWidth, height: node.clientHeight,
    }));
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  return (
    <div ref={host} className={`smooth-host ${className}`}>
      <InputSurface className="preview-surface">
        <svg className="surface-svg" width={size.width} height={size.height} aria-hidden="true">
          <path className="surface-shape" d={cornerPath(size.width, size.height, radius, smoothing)} />
        </svg>
        <div className="surface-content">{children}</div>
      </InputSurface>
    </div>
  );
}

function App() {
  const [mode, setMode] = useState<"continuous" | "circular">("continuous");
  const [radius, setRadius] = useState(24);
  const [smoothing, setSmoothing] = useState(60);
  const [drawing, setDrawing] = useState(0.6);
  const [text, setText] = useState("");
  const [outline, setOutline] = useState(false);
  const progress = useRef({ value: 0.6 });
  const footerRadius = Math.round(radius * 20 / 24);
  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const target = mode === "continuous" ? smoothing / 100 : 0;
    let tween: gsap.core.Tween | undefined;
    const settle = () => {
      if (!media.matches) return;
      tween?.kill();
      progress.current.value = target;
      setDrawing(target);
    };
    tween = gsap.to(progress.current, {
      value: target, duration: prefersReducedMotion() ? 0 : 0.24, ease: "power2.out",
      onUpdate: () => setDrawing(progress.current.value),
    });
    media.addEventListener("change", settle);
    return () => { tween?.kill(); media.removeEventListener("change", settle); };
  }, [mode, smoothing]);

  const reset = () => { setRadius(24); setSmoothing(60); setMode("continuous"); };
  return (
    <div className={`corner-demo ${outline ? "show-outlines" : ""}`}>
      <aside className="preview-sidebar" aria-label="首页背景示意">
        <div className="sidebar-brand"><PanelLeft size={19} /><span>DeepAgent</span></div>
        <div className="nav-item selected"><SquarePen />新对话</div>
        <div className="nav-item"><Search />搜索</div>
        <div className="nav-item"><Layers />技能</div>
        <div className="nav-item"><Library />知识库</div>
        <div className="nav-item"><MoreHorizontal />其他<ChevronRight className="nav-end" /></div>
        <div className="sidebar-group">项目<ChevronDown size={13} /></div>
        <div className="sidebar-empty">暂无项目</div>
        <div className="sidebar-group">最近对话<ChevronDown size={13} /></div>
        <div className="sidebar-empty">暂无对话</div>
        <div className="nav-item sidebar-settings"><Settings />设置</div>
      </aside>

      <main className="preview-main">
        <header className="preview-header">
          <div className="preview-heading"><strong>连续圆角</strong><span>DeepAgent · 外观预览</span></div>
          <div className="mode-switch" role="group" aria-label="选择圆角风格">
            <Button variant="ghost" className="mode-button" aria-pressed={mode === "circular"}
              onClick={() => setMode("circular")}>当前圆角</Button>
            <Button variant="ghost" className="mode-button" aria-pressed={mode === "continuous"}
              onClick={() => setMode("continuous")}>苹果风格</Button>
          </div>
        </header>

        <section className="preview-stage" aria-label="输入框与底部工具栏预览">
          <div className="composer-example">
            <p className="preview-eyebrow"><span />{mode === "continuous" ? "连续曲线 · 预览中" : "普通圆弧 · 对比中"}</p>
            <h1>我们应该做些什么？</h1>
            <Surface radius={radius} smoothing={drawing} className="composer-surface">
              <textarea aria-label="试试输入内容" placeholder="尽管问..." value={text}
                onChange={event => setText(event.target.value)} />
              <div className="composer-actions">
                <span className="round-control" aria-label="添加附件示意"><Plus size={18} /></span>
                <span className="preview-chip"><Hand size={15} />请求批准<ChevronDown size={12} /></span>
                <span className="actions-spacer" />
                <span className="capacity-ring" aria-label="上下文容量示意" />
                <span className="preview-chip model-chip"><Brain size={16} /><b>deepseek-flash</b><span>深度</span><ChevronDown size={12} /></span>
                <span className={`round-control send-control ${text.trim() ? "has-input" : ""}`} aria-label="发送按钮外观示意"><ArrowUp size={18} /></span>
              </div>
            </Surface>
            <Surface radius={footerRadius} smoothing={drawing} className="footer-surface">
              <div className="composer-footer">
                <span><Folder size={16} />不使用项目<ChevronDown size={12} /></span>
                <span><Monitor size={16} />本地模式<ChevronDown size={12} /></span>
                <span className="balance"><Wallet size={14} />余额 ¥0.66</span>
              </div>
            </Surface>
            <p className="current-spec" aria-live="polite">输入框 {radius}px<span>·</span>工具栏 {footerRadius}px<span>·</span>平滑度 {mode === "continuous" ? smoothing : 0}%</p>
          </div>
        </section>

        <section className="comparison-panel" aria-label="圆角细节与调节">
          <div className="corner-detail">
            <div className="corner-zoom" aria-label="同半径下，灰色普通圆角与绿色连续圆角的放大叠加图">
              <svg viewBox="0 0 104 100" aria-hidden="true">
                <g transform="translate(-116 10)">
                  <path d={cornerPath(210, 180, 72, 0)} className="circular-outline" />
                  <path d={cornerPath(210, 180, 72, smoothing / 100)} className="continuous-outline" />
                </g>
              </svg>
              <span>3×</span>
            </div>
            <div className="detail-copy"><h2>让直边自然过渡到弯角</h2>
              <p>保持同样的圆角半径，只改变曲线的衔接。</p>
              <div className="outline-legend"><span><i />普通圆角</span><span><i />连续圆角</span></div>
            </div>
          </div>
          <div className="adjustments">
            <div className="slider-control"><label id="radius-label">圆角大小<output>{radius}px</output></label>
              <Slider className="preview-slider" aria-labelledby="radius-label" thumbProps={{ "aria-labelledby": "radius-label" }}
                min={16} max={36} step={1} value={[radius]} onValueChange={([value]) => setRadius(value)} />
            </div>
            <div className="slider-control"><label id="smoothing-label">平滑度<output>{smoothing}%</output></label>
              <Slider className="preview-slider" aria-labelledby="smoothing-label" thumbProps={{ "aria-labelledby": "smoothing-label" }}
                min={0} max={100} step={5} value={[smoothing]} onValueChange={([value]) => { setSmoothing(value); setMode("continuous"); }} />
            </div>
            <div className="adjustment-actions">
              <Button className="detail-button" variant="ghost" aria-pressed={outline} onClick={() => setOutline(value => !value)}>显示轮廓</Button>
              <Button className="detail-button reset-button" variant="ghost" onClick={reset}>恢复推荐值</Button>
            </div>
          </div>
        </section>
        <footer className="preview-note">苹果风格的网页近似效果 · 仅此预览页面生效<span>圆形按钮与胶囊保持原形</span></footer>
      </main>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
