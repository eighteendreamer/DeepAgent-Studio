import { HoverInfo } from "./ui/HoverInfo";
import { ArrowLeft, ArrowRight, Copy, Minus, PanelLeft, Square, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { isMainWindow, NAV_ROW_ID } from "./decorumTitlebar";
import { Button } from "./shadcn/button";
import { cn } from "./shadcn/utils";
import { message } from "./message";

interface Props {
  onToggleSidebar: () => void;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
  className?: string;
}

const BUTTON_CLASS =
  "app-titlebar-button text-text-secondary transition-colors hover:bg-sidebar-highlight hover:text-text-base disabled:cursor-default disabled:opacity-35 disabled:hover:bg-transparent";

export function CustomTitleBar({
  onToggleSidebar,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
  className,
}: Props) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(true);
  const [nativeWindow, setNativeWindow] = useState<import("@tauri-apps/api/window").Window | null>(null);
  const [maximized, setMaximized] = useState(false);
  const snapTimer = useRef<ReturnType<typeof setTimeout>>();
  const cancelSnap = () => clearTimeout(snapTimer.current);
  useEffect(() => cancelSnap, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void isMainWindow().then(ok => { if (!disposed) setVisible(ok); });
    if ("__TAURI_INTERNALS__" in window) {
      void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
        const current = getCurrentWindow();
        if (disposed || current.label !== "main") return;
        setNativeWindow(current);
        const refresh = async () => {
          const value = await current.isMaximized();
          if (!disposed) setMaximized(value);
        };
        await refresh();
        const stop = await current.onResized(() => { void refresh().catch(error => console.error("titlebar resize", error)); });
        if (disposed) stop(); else unlisten = stop;
      }).catch(error => console.error("titlebar initialization", error));
    }
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const nativeAction = (action: "minimize" | "toggleMaximize" | "close") => {
    cancelSnap();
    if (nativeWindow) void nativeWindow[action]().catch(error => message.error(String(error)));
  };
  const scheduleSnap = () => {
    cancelSnap();
    if (nativeWindow && navigator.userAgent.includes("Windows")) {
      // Preserve decorum's existing Windows snap-layout hover behavior.
      snapTimer.current = setTimeout(() => {
        void nativeWindow.setFocus().then(() => invoke("plugin:decorum|show_snap_overlay"))
          .catch(error => console.error("titlebar snap overlay", error));
      }, 620);
    }
  };

  if (!visible) return null;

  const navigation = [
    { key: "toggleSidebar", icon: PanelLeft, onClick: onToggleSidebar, disabled: false },
    { key: "goBack", icon: ArrowLeft, onClick: onBack, disabled: !canGoBack },
    { key: "goForward", icon: ArrowRight, onClick: onForward, disabled: !canGoForward },
  ];

  return (
    <header className="app-titlebar app-titlebar-spacer shrink-0">
      <div id={NAV_ROW_ID} className={cn("app-titlebar-navigation", className)}>
        {navigation.map(item => <HoverInfo key={item.key} content={t(`settings.shortcuts.${item.key}`)}>
          <Button variant="ghost" size="icon" className={BUTTON_CLASS} onClick={item.onClick}
            disabled={item.disabled} aria-label={t(`settings.shortcuts.${item.key}`)}>
            <item.icon />
          </Button>
        </HoverInfo>)}
      </div>
      <div className="h-full flex-1" data-tauri-drag-region />
      {nativeWindow && <div className="app-window-controls">
        <Button variant="ghost" size="icon" aria-label={t("titleBar.minimize")} onClick={() => nativeAction("minimize")}><Minus /></Button>
        <Button variant="ghost" size="icon" aria-label={t(maximized ? "titleBar.restore" : "titleBar.maximize")} onMouseEnter={scheduleSnap} onMouseLeave={cancelSnap} onClick={() => nativeAction("toggleMaximize")}>{maximized ? <Copy /> : <Square />}</Button>
        <Button variant="ghost" size="icon" className="app-window-close" aria-label={t("titleBar.close")} onClick={() => nativeAction("close")}><X /></Button>
      </div>}
    </header>
  );
}
