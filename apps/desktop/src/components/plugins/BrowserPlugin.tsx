import { useEffect, useMemo, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

interface BrowserPluginProps {
  initialUrl?: string;
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

export function BrowserPlugin({ initialUrl = "" }: BrowserPluginProps) {
  const [url, setUrl] = useState(() => normalizeUrl(initialUrl));
  const [draftUrl, setDraftUrl] = useState(() => normalizeUrl(initialUrl));
  const [reloadKey, setReloadKey] = useState(0);
  const [nativeError, setNativeError] = useState<string | null>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const webviewRef = useRef<any>(null);
  const labelRef = useRef("");
  const canUseNativeWebview = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  const host = useMemo(() => titleFromUrl(url), [url]);

  useEffect(() => {
    const next = normalizeUrl(initialUrl);
    setUrl(next);
    setDraftUrl(next);
  }, [initialUrl]);

  useEffect(() => {
    if (!canUseNativeWebview || !url) return;
    const container = contentRef.current;
    if (!container) return;

    let cancelled = false;
    let resizeObserver: ResizeObserver | null = null;

    const syncBounds = async () => {
      const webview = webviewRef.current;
      const el = contentRef.current;
      if (!webview || !el) return;
      try {
        const rect = el.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) {
          await webview.hide?.();
          return;
        }
        const { LogicalPosition, LogicalSize } = await import("@tauri-apps/api/dpi");
        await webview.setPosition(new LogicalPosition(rect.left, rect.top));
        await webview.setSize(new LogicalSize(rect.width, rect.height));
        await webview.show?.();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        if (!/webview not found/i.test(message)) {
          setNativeError(message);
        }
      }
    };

    const createWebview = async () => {
      try {
        setNativeError(null);
        await webviewRef.current?.close?.().catch(() => {});
        webviewRef.current = null;

        const [{ Webview }, { getCurrentWindow }] = await Promise.all([
          import("@tauri-apps/api/webview"),
          import("@tauri-apps/api/window"),
        ]);
        if (cancelled || !contentRef.current) return;

        const rect = contentRef.current.getBoundingClientRect();
        labelRef.current = `browser_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
        const webview = new Webview(getCurrentWindow(), labelRef.current, {
          url,
          x: rect.left,
          y: rect.top,
          width: Math.max(1, rect.width),
          height: Math.max(1, rect.height),
          backgroundColor: [255, 255, 255, 255],
          focus: true,
        });
        webview.once("tauri://created", () => {
          webviewRef.current = webview;
          void webview.setBackgroundColor?.([255, 255, 255, 255]).catch(() => {});
          void syncBounds();
        });
        webview.once("tauri://error", (event: any) => {
          if (webviewRef.current === webview) webviewRef.current = null;
          setNativeError(String(event?.payload ?? "内置浏览器创建失败"));
        });
      } catch (error) {
        setNativeError(error instanceof Error ? error.message : String(error));
      }
    };

    void createWebview();
    resizeObserver = new ResizeObserver(() => {
      void syncBounds();
    });
    resizeObserver.observe(container);
    window.addEventListener("resize", syncBounds);

    return () => {
      cancelled = true;
      resizeObserver?.disconnect();
      window.removeEventListener("resize", syncBounds);
      const webview = webviewRef.current;
      webviewRef.current = null;
      void webview?.close?.().catch(() => {});
    };
  }, [canUseNativeWebview, reloadKey, url]);

  const navigate = () => {
    const next = normalizeUrl(draftUrl);
    setDraftUrl(next);
    setUrl(next);
  };

  return (
    <div className="w-full h-full flex flex-col bg-white">
      <div className="flex items-center gap-3 px-4 py-2 border-b border-border-theme flex-shrink-0">
        <div className="flex items-center gap-3 text-text-secondary">
          <FontAwesomeIcon icon={["fas", "arrow-left"]} className="cursor-not-allowed opacity-40" />
          <FontAwesomeIcon icon={["fas", "arrow-right"]} className="cursor-not-allowed opacity-40" />
          <button
            type="button"
            className="text-text-secondary hover:text-text-base"
            title="重新加载"
            onClick={() => setReloadKey((current) => current + 1)}
          >
            <FontAwesomeIcon icon={["fas", "rotate-right"]} className="text-[13px]" />
          </button>
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
        {host && (
          <div className="hidden max-w-[160px] truncate text-[12px] text-text-secondary md:block" title={host}>
            {host}
          </div>
        )}
      </div>

      <div ref={contentRef} className="relative flex-1 overflow-hidden bg-white">
        {!url ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-white px-6 text-center">
            <FontAwesomeIcon icon={["fas", "globe"]} className="mb-3 text-2xl text-text-secondary" />
            <div className="mb-1 text-[14px] font-medium text-text-base">浏览器</div>
            <div className="text-[12px] text-text-secondary">输入 URL 后打开页面</div>
          </div>
        ) : !canUseNativeWebview && (
          <iframe title="browser" src={url} className="h-full w-full border-0 bg-white" />
        )}
        {nativeError && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-white px-6 text-center">
            <FontAwesomeIcon icon={["fas", "globe"]} className="mb-3 text-2xl text-text-secondary" />
            <div className="mb-2 text-[14px] font-medium text-text-base">内置浏览器打开失败</div>
            <div className="max-w-sm text-[12px] leading-relaxed text-text-secondary">{nativeError}</div>
          </div>
        )}
      </div>
    </div>
  );
}
