import { HoverInfo } from "../ui/HoverInfo";
import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "../shadcn/button";
import { ArrowLeft, ArrowRight, Globe, RotateCw } from "lucide-react";
import type { PluginDefinition } from "./pluginTypes";

interface BrowserPluginProps {
  initialUrl?: string;
  sessionId?: string | null;
}

function normalizeUrl(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return "";
  if (/^https?:\/\//i.test(trimmed)) return trimmed;
  if (/^(localhost|127\.0\.0\.1)(:\d+)?(\/.*)?$/i.test(trimmed)) {
    return `http://${trimmed}`;
  }
  return `https://${trimmed}`;
}

function titleFromUrl(url: string): string {
  if (!url) return "";
  try {
    return new URL(normalizeUrl(url)).host;
  } catch {
    return url;
  }
}

export function BrowserPlugin({ initialUrl = "", sessionId }: BrowserPluginProps) {
  const [url, setUrl] = useState(() => normalizeUrl(initialUrl));
  const [draftUrl, setDraftUrl] = useState(() => normalizeUrl(initialUrl));
  const [reloadKey, setReloadKey] = useState(0);
  const [nativeError, setNativeError] = useState<string | null>(null);
  const [tabId, setTabId] = useState<string | null>(null);
  const [granted, setGranted] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);
  const canUseNativeWebview = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const host = useMemo(() => titleFromUrl(url), [url]);

  useEffect(() => {
    const next = normalizeUrl(initialUrl);
    setUrl(next);
    setDraftUrl(next);
    setGranted(false);
    setTabId(null);
    if (!canUseNativeWebview || !contentRef.current) return;
    const container = contentRef.current;
    let disposed = false;
    let label: string | null = null;
    let unlisten: (() => void) | undefined;
    let frame = 0;
    const bounds = () => {
      const { x, y, width, height } = container.getBoundingClientRect();
      return { x, y, width, height };
    };
    const sync = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (!disposed && label) void invoke("browser_tab_bounds", { id: label, bounds: bounds() }).catch(report);
      });
    };
    function report(error: unknown) {
      if (!disposed) setNativeError(String(error));
    }
    const observer = new ResizeObserver(sync);
    observer.observe(container);
    window.addEventListener("resize", sync);
    void (async () => {
      setNativeError(null);
      unlisten = await listen<{ id: string; url: string }>("workspace-browser://location", ({ payload }) => {
        if (payload.id !== label || payload.url === "about:blank") return;
        setUrl(payload.url);
        setDraftUrl(payload.url);
      });
      if (disposed) { unlisten(); return; }
      label = await invoke<string>("browser_tab_open", { sessionId: sessionId ?? null, url: next, bounds: bounds() });
      if (disposed) { await invoke("browser_tab_close", { id: label }); return; }
      setTabId(label);
      sync();
    })().catch(report);
    return () => {
      disposed = true;
      observer.disconnect();
      window.removeEventListener("resize", sync);
      cancelAnimationFrame(frame);
      unlisten?.();
      if (label) void invoke("browser_tab_close", { id: label }).catch(error => {
        console.warn("Could not close workspace browser tab", error);
      });
    };
  }, [canUseNativeWebview, initialUrl, sessionId]);

  const navigate = (reload = false) => {
    const next = reload ? url : normalizeUrl(draftUrl);
    if (!next) return;
    setNativeError(null);
    if (canUseNativeWebview && tabId) {
      void invoke("browser_tab_navigate", { id: tabId, url: reload ? null : next })
        .catch(error => setNativeError(String(error)));
    } else if (!canUseNativeWebview) {
      setDraftUrl(next);
      setUrl(next);
      if (reload) setReloadKey(key => key + 1);
    }
  };

  const setAccess = () => {
    if (!tabId) return;
    void invoke("browser_tab_access", { id: tabId, enabled: !granted }).then(() => {
      setGranted(!granted);
      setNativeError(null);
    }).catch(error => setNativeError(String(error)));
  };

  return (
    <div className="w-full h-full flex flex-col bg-white">
      <div className="flex items-center gap-3 px-4 py-2 border-b border-border-theme flex-shrink-0">
        <div className="flex items-center gap-3 text-text-secondary">
          <ArrowLeft className="h-4 w-4 cursor-not-allowed opacity-40" aria-hidden="true" />
          <ArrowRight className="h-4 w-4 cursor-not-allowed opacity-40" aria-hidden="true" />
          <HoverInfo content="重新加载"><button
            type="button"
            className="text-text-secondary hover:text-text-base"

            onClick={() => navigate(true)}
          >
            <RotateCw className="h-4 w-4" aria-hidden="true" />
          </button></HoverInfo>
        </div>
        <form
          className="flex-1"
          onSubmit={(event) => {
            event.preventDefault();
            navigate();
          }}
        >
          <input
            value={draftUrl}
            onChange={(event) => setDraftUrl(event.target.value)}
            onBlur={() => setDraftUrl(draftUrl.trim() ? normalizeUrl(draftUrl) : "")}
            className="w-full rounded-lg border border-border-theme bg-gray-50 px-3 py-1.5 text-[13px] text-text-base outline-none focus:border-blue-400 focus:bg-white"
            placeholder="输入 URL"
            spellCheck={false}
            aria-label="浏览器地址"
          />
        </form>
        {sessionId && tabId && <Button type="button" variant={granted ? "secondary" : "outline"} size="sm"
          aria-pressed={granted} onClick={setAccess}>{granted ? "停止 AI 控制" : "允许 AI 控制"}</Button>}
        {host && (
          <HoverInfo content={host}><div className="hidden max-w-[160px] truncate text-[12px] text-text-secondary md:block" >
            {host}
          </div></HoverInfo>
        )}
      </div>

      <div ref={contentRef} className="relative flex-1 overflow-hidden bg-white">
        {!url ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-white px-6 text-center">
            <Globe className="mb-3 h-8 w-8 text-text-secondary" aria-hidden="true" />
            <div className="mb-1 text-[14px] font-medium text-text-base">浏览器</div>
            <div className="text-[12px] text-text-secondary">输入 URL 后打开页面</div>
          </div>
        ) : !canUseNativeWebview && (
          <iframe key={reloadKey} title="browser" src={url} className="h-full w-full border-0 bg-white" />
        )}
        {nativeError && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-white px-6 text-center">
            <Globe className="mb-3 h-8 w-8 text-text-secondary" aria-hidden="true" />
            <div className="mb-2 text-[14px] font-medium text-text-base">内置浏览器打开失败</div>
            <div className="max-w-sm text-[12px] leading-relaxed text-text-secondary">{nativeError}</div>
          </div>
        )}
      </div>
    </div>
  );
}

export const browserPluginDefinition: PluginDefinition = {
  type: "browser",
  icon: ["fas", "globe"],
  titleKey: "browser",
  descKey: "browserDesc",
  fallbackTitle: "Browser",
  fallbackDesc: "Open website",
  getTabTitle: ({ t }, options) =>
    options?.url
      ? titleFromUrl(options.url)
      : t?.("chatView.tools.browser", { defaultValue: "Browser" }) || "Browser",
  render: ({ tab }) => <BrowserPlugin initialUrl={tab.url} />,
};
