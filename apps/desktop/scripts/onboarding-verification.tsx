// Dev-only fixture: the production wizard and API bridge, with Tauri's official IPC mock.
// Served on a separate localhost origin; never uses a real key or backend settings.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import "../src/i18n";
import "../src/icons";
import "../src/styles.css";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { OnboardingWizard } from "../src/components/OnboardingWizard";
import { Button } from "../src/components/shadcn/button";

let scenario = "retry", failSave = false, connectCalls = 0, saveCalls = 0;
let notify = () => {};
let releasePending: (() => void) | undefined;
const delay = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
mockIPC(async (command, args) => {
  if (command === "get_welcome_name") return "程序员Eighteen";
  if (command === "set_window_translucent") return;
  if (command === "initialize_project") {
    const attempt = ++connectCalls;
    notify();
    if (scenario === "pending") await new Promise<void>(resolve => { releasePending = resolve; });
    else await delay(650);
    if (scenario === "failure" || (scenario === "retry" && attempt === 1)) throw new Error("fixture: connection rejected");
    return {};
  }
  if (command === "set_sandbox_mode") {
    saveCalls++;
    notify();
    await delay(350);
    if (failSave) throw new Error("fixture: settings save rejected");
    document.querySelector("#saved-mode")!.textContent = String((args as { mode: string }).mode);
    return {};
  }
  throw new Error(`Unexpected verification IPC: ${command}`);
});
addEventListener("pagehide", () => clearMocks());

function Verification() {
  const [round, setRound] = useState(0);
  const [complete, setComplete] = useState(false);
  const [, update] = useState(0);
  notify = () => update(value => value + 1);
  return (
    <ThemeProvider>
      <div className="relative h-screen bg-bg-base text-text-base">
        <div className="h-8 bg-sidebar-bg px-4 text-xs leading-8 text-text-secondary">DeepAgent Studio · 正式初始化组件验证</div>
        <div className="absolute inset-x-0 bottom-16 top-8 flex">
          <aside className="w-56 bg-sidebar-bg p-6 text-sm">项目<br /><br />DeepAgent Studio<br /><br />最近对话</aside>
          <main className="flex flex-1 flex-col items-center justify-center gap-4"><h2 className="text-3xl font-semibold">今天有什么计划？</h2><p>{complete ? "初始化完成" : "工作台已就绪"}</p></main>
        </div>
        {!complete && <OnboardingWizard key={round} onComplete={() => setComplete(true)} />}
        <div className="fixed inset-x-0 bottom-0 z-[21000] flex min-h-16 flex-wrap items-center gap-3 bg-bg-base p-3 text-xs">
          <select aria-label="验证连接结果" defaultValue="retry" onChange={event => { scenario = event.target.value; }} className="rounded border border-border-theme bg-bg-base p-1.5">
            <option value="retry">首次失败，重试成功</option><option value="success">成功</option><option value="failure">失败</option><option value="pending">挂起，手动完成</option>
          </select>
          <label><input type="checkbox" onChange={event => { failSave = event.target.checked; }} /> 模拟保存失败</label>
          <Button size="sm" variant="outline" onClick={() => releasePending?.()}>完成挂起请求</Button>
          <Button size="sm" variant="outline" onClick={() => { connectCalls = 0; saveCalls = 0; setComplete(false); setRound(value => value + 1); }}>重置向导</Button>
          <span role="status">连接请求：{connectCalls} · 保存请求：{saveCalls} · 完成：{String(complete)}</span>
          <span id="saved-mode" />
        </div>
      </div>
    </ThemeProvider>
  );
}

createRoot(document.querySelector("#root")!).render(<React.StrictMode><Verification /></React.StrictMode>);
