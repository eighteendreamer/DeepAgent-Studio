import { HoverInfo } from "./ui/HoverInfo";
import { ArrowLeft, ArrowRight, PanelLeft } from "lucide-react";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import {
  isMainWindow,
  NAV_ROW_ID,
  NAV_SLOT_ID,
  waitForDecorumElement,
} from "./decorumTitlebar";
import { Button } from "./shadcn/button";
import { cn } from "./shadcn/utils";

interface Props {
  onToggleSidebar: () => void;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
  className?: string;
}

const BUTTON_CLASS =
  "flex h-7 w-7 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-sidebar-highlight hover:text-text-base disabled:cursor-default disabled:opacity-35 disabled:hover:bg-transparent";

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
  const [slot, setSlot] = useState<HTMLElement | null>(null);

  useEffect(() => {
    let disposed = false;
    isMainWindow()
      .then((ok) => {
        if (!disposed) setVisible(ok);
      })
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (!visible) return;
    let disposed = false;
    waitForDecorumElement("[data-tauri-decorum-tb]")
      .then((titlebar) => {
        if (disposed) return;
        let el = document.getElementById(NAV_SLOT_ID) as HTMLDivElement | null;
        if (!el) {
          el = document.createElement("div");
          el.id = NAV_SLOT_ID;
          // 容器是 justify-content:end 的 flex，流内排布只会贴右侧；
          // 绝对定位钉在最左，且定位节点绘制在流内拖拽层之上，保证可点击。
          el.style.position = "absolute";
          el.style.left = "0";
          el.style.top = "0";
          el.style.height = "100%";
          const minimizeBtn = titlebar.querySelector("#decorum-tb-minimize");
          if (minimizeBtn) {
            titlebar.insertBefore(el, minimizeBtn);
          } else {
            titlebar.appendChild(el);
          }
        }
        setSlot(el);
      })
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, [visible]);

  useEffect(() => {
    return () => {
      document.getElementById(NAV_SLOT_ID)?.remove();
    };
  }, []);

  if (!visible) return null;

  return (
    <>
      {/* 32px 占位：根容器是 flex-col，把内容区压到标题栏下方；session 窗口不渲染即不受影响 */}
      <div className="h-8 shrink-0" />
      {slot &&
        createPortal(
          <div
            id={NAV_ROW_ID}
            className={cn("flex h-full items-center gap-1 pl-3", className)}
          >
            <HoverInfo content={t("settings.shortcuts.toggleSidebar")}><Button
              variant="ghost"
              size="icon"
              className={BUTTON_CLASS}
              onClick={onToggleSidebar}

              aria-label={t("settings.shortcuts.toggleSidebar")}
            >
              <PanelLeft className="h-4 w-4" />
            </Button></HoverInfo>
            <HoverInfo content={t("settings.shortcuts.goBack")}><Button
              variant="ghost"
              size="icon"
              className={BUTTON_CLASS}
              onClick={onBack}
              disabled={!canGoBack}

              aria-label={t("settings.shortcuts.goBack")}
            >
              <ArrowLeft className="h-4 w-4" />
            </Button></HoverInfo>
            <HoverInfo content={t("settings.shortcuts.goForward")}><Button
              variant="ghost"
              size="icon"
              className={BUTTON_CLASS}
              onClick={onForward}
              disabled={!canGoForward}

              aria-label={t("settings.shortcuts.goForward")}
            >
              <ArrowRight className="h-4 w-4" />
            </Button></HoverInfo>
          </div>,
          slot,
        )}
    </>
  );
}
