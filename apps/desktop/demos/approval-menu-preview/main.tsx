import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { ArrowUp, BookOpen, Check, ChevronDown, ChevronRight, CircleAlert, Folder, Hand, Layers, MessageSquarePlus, Monitor, MoreHorizontal, Plus, RotateCcw, Search, Settings, ShieldHalf, Sparkles } from "lucide-react";
import { Button } from "../../src/components/shadcn/button";
import { MorphingMenuShell } from "../../src/components/ui/MorphingMenuShell";
import { SlidingMenuList } from "../../src/components/ui/SlidingMenuList";
import zh from "../../src/locales/zh.json";

const copy = zh.composer;
const options = [
  { id: "default", label: copy.defaultPermission, description: copy.defaultPermissionDesc, Icon: Hand },
  { id: "auto", label: copy.autoReview, description: copy.autoReviewDesc, Icon: ShieldHalf },
  { id: "full", label: copy.fullAccess, description: copy.fullAccessDesc, Icon: CircleAlert },
] as const;
type Mode = "before" | "after";

function Preview() {
  const [mode, setMode] = useState<Mode>("after");
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<(typeof options)[number]>(options[0]);
  const replayTimer = useRef<number>();
  useEffect(() => {
    replayTimer.current = window.setTimeout(() => setOpen(true), 220);
    return () => window.clearTimeout(replayTimer.current);
  }, []);

  const replay = (nextMode = mode) => {
    window.clearTimeout(replayTimer.current);
    setOpen(false);
    replayTimer.current = window.setTimeout(() => {
      setMode(nextMode);
      // Let the closed button settle before measuring its original geometry.
      replayTimer.current = window.setTimeout(() => setOpen(true), 80);
    }, 720);
  };

  return <div className="approval-preview" data-preview-mode={mode}>
    <header className="preview-header">
      <div className="preview-heading"><span className="preview-eyebrow">交互预览</span><h1>审批菜单</h1></div>
      <div className="preview-actions">
        <div className="comparison-switch" role="group" aria-label="对比版本">
          <Button variant="ghost" className="comparison-option" aria-pressed={mode === "before"} onClick={() => replay("before")}>修改前</Button>
          <Button variant="ghost" className="comparison-option" aria-pressed={mode === "after"} onClick={() => replay("after")}>优化后</Button>
        </div>
        <Button variant="ghost" className="replay-control" onClick={() => replay()} aria-label="重播展开动画"><RotateCcw size={14}/><span>重播</span></Button>
      </div>
    </header>

    <section className="preview-workbench" aria-label="聊天输入区中的审批菜单">
      <aside className="preview-sidebar" aria-hidden="true">
        <div className="sidebar-top"><MessageSquarePlus size={17}/>新对话</div>
        <div><Search size={17}/>搜索</div><div><Layers size={17}/>技能</div><div><BookOpen size={17}/>知识库</div>
        <div><MoreHorizontal size={17}/>其他<ChevronRight size={13} className="push-right"/></div>
        <span className="sidebar-section">项目 <ChevronDown size={11}/></span><span className="sidebar-empty">暂无项目</span>
        <span className="sidebar-section">最近对话 <ChevronDown size={11}/></span><span className="sidebar-empty">暂无对话</span>
        <div className="sidebar-settings"><Settings size={17}/>设置</div>
      </aside>
      <main className={`preview-main ${mode}`}>
        <h2>我们应该做些什么？</h2>
        <div className="composer-stack">
          <div className="composer-card">
            <div className="composer-input" aria-hidden="true">尽管问...</div>
            <div className="composer-toolbar">
              <span className="mock-add" aria-hidden="true"><Plus size={18}/></span>
              <MorphingMenuShell open={open} onOpenChange={setOpen} layoutId="approval-preview"
                panelClassName={`approval-menu ${mode}`} panelAlign="left"
                panelShadow={mode === "after" ? "0 4px 12px rgba(0,0,0,0.08)" : undefined}
                trigger={<Button variant="ghost" size="sm" className={`approval-trigger ${selected.id === "full" ? "full-selected" : ""}`}
                  aria-label="审批模式" aria-expanded={open} aria-haspopup="menu" onClick={() => setOpen(value => !value)}>
                  <selected.Icon size={12}/><span>{selected.label}</span><ChevronDown size={10}/>
                </Button>}>
                <div className="approval-menu-padding">
                  <SlidingMenuList activeId={selected.id} pillClassName={`approval-selection ${selected.id === "full" ? "full-selected" : ""}`}
                    className="approval-options" role="menu" aria-label="审批模式选项" onKeyDown={event => {
                      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
                      event.preventDefault();
                      const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("[role=menuitemradio]"));
                      const current = items.indexOf(document.activeElement as HTMLButtonElement);
                      const index = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : event.key === "ArrowDown" ? (current + 1) % items.length : (current - 1 + items.length) % items.length;
                      items[index]?.focus();
                    }}>
                    {options.map(option => <Button key={option.id} variant="ghost" data-menu-item={option.id}
                      role="menuitemradio" aria-checked={selected.id === option.id}
                      className={`approval-option ${selected.id === option.id && option.id === "full" ? "full-selected" : ""}`}
                      onClick={() => { setSelected(option); setOpen(false); }}>
                      <option.Icon size={14} className="approval-option-icon"/>
                      <span className="approval-option-copy"><span className="approval-option-title">{option.label}</span><span className="approval-option-description">{option.description}</span></span>
                      {selected.id === option.id && <Check size={11} className="approval-option-check"/>}
                    </Button>)}
                  </SlidingMenuList>
                </div>
              </MorphingMenuShell>
              <div className="composer-right" aria-hidden="true"><span className="context-ring"/><span className="mock-model"><Sparkles size={12}/><b>deepseek-flash</b><span>深度</span><ChevronDown size={10}/></span><span className="mock-send"><ArrowUp size={16}/></span></div>
            </div>
          </div>
          <div className="composer-bottom" aria-hidden="true"><span><Folder size={14}/>不使用项目<ChevronDown size={10}/></span><span><Monitor size={14}/>本地模式<ChevronDown size={10}/></span><small>余额 ¥0.66</small></div>
        </div>
      </main>
    </section>
    <footer className="preview-footer"><span>{mode === "after" ? "288 px · 标题 12 px · 说明 11 px · 连续圆角" : "380 px · 当前项目中的菜单尺寸与间距"}</span><span>点空白收起，再点「{selected.label}」展开；可切换三种模式</span></footer>
  </div>;
}

createRoot(document.getElementById("root")!).render(<Preview/>);
