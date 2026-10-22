import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import zh from "../../../apps/desktop/src/locales/zh.json";
import { AppNavigationRail } from "../../../apps/desktop/src/components/AppNavigationRail";
import { Button } from "../../../apps/desktop/src/components/shadcn/button";

i18n.use(initReactI18next).init({ lng: "zh", resources: { zh: { translation: zh } }, interpolation: { escapeValue: false } });
type Surface = React.ComponentProps<typeof AppNavigationRail>["activeSurface"];
function Applied() {
  const [active, setActive] = useState<Surface>("start");
  const [sidebar, setSidebar] = useState(true), [dark, setDark] = useState(false), [status, setStatus] = useState("首页");
  const select = (surface: Surface, name: string) => { setActive(surface); setStatus(name); };
  return <main className={dark ? "applied-review dark" : "applied-review"}>
    <header className="applied-header"><p>DEEPAGENT STUDIO · E</p><h1>轻弹即停 · 接入效果</h1><span>220 ms · back.out(0.7) · 移出 70 ms · 提示延迟 160 ms</span><Button variant="outline" onClick={() => { document.documentElement.classList.toggle("dark", !dark); setDark(!dark); }}>{dark ? "浅色预览" : "深色预览"}</Button></header>
    <section className="applied-window"><div className="app-workspace">
      <AppNavigationRail activeSurface={active} sidebarOpen={sidebar}
        onToggleSidebar={() => setSidebar(!sidebar)} onHome={() => select("start", "首页")}
        onNewChat={() => select("chat", "新对话")} onOpenSearch={() => setStatus("搜索")}
        onOpenSkills={() => select("skills", "技能")} onOpenKnowledge={() => select("knowledge", "知识库")}
        onOpenPlugins={() => select("plugins", "插件")} onOpenAutomation={() => select("automation", "自动化")}
        onOpenRemote={() => select("remote", "远程")} onOpenCanvas={() => select("canvas", "画布")}
        onOpenSettings={() => select("settings", "设置")} onLogout={() => setStatus("退出回调")} />
      <div className="app-content-frame">{sidebar && <aside className="applied-sidebar"><strong>DeepAgent</strong><p>新对话</p><span>项目</span><small>暂无项目</small><span>最近</span><small>暂无对话</small></aside>}
        <div className="applied-pane"><h2>{status}</h2><p>沿左侧图标移动，观察高亮滑动与快速折返</p><p>本页直接使用桌面端导航组件，按钮行为用于预览</p></div>
      </div>
    </div></section>
  </main>;
}
createRoot(document.getElementById("root")!).render(<React.StrictMode><Applied /></React.StrictMode>);
