import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { CircleUser, LogOut, Settings } from "lucide-react";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";
import { MENU_LIST, MOTION } from "./ui/motion";
import { SpringSurface } from "./ui/SpringSurface";
import { MorphPanelLayoutContext } from "./ui/MorphPanelLayoutContext";
import { cn } from "./shadcn/utils";
import { Button } from "./shadcn/button";

const PANEL_WIDTH = 224;
const PANEL_GAP = 6;

const SETTINGS_MENU = {
  pad: "px-2 py-1.5",
  row: "relative z-[1] flex w-full cursor-pointer items-center rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base whitespace-nowrap hover:bg-transparent",
  icon: "mr-2.5 h-4 w-4 shrink-0 text-text-secondary",
  pill: "left-0 right-0 rounded-lg",
  account: "flex min-w-0 items-center px-2.5 py-2 text-[13px] font-medium text-text-base",
} as const;

type Anchor = { left: number; bottom: number };

type Props = {
  onOpenSettings: () => void;
  onLogout: () => void;
  compact?: boolean;
};

/** 面板底边对齐触发器顶边上方，左缘与触发器左缘对齐 */
function anchorPanelAboveTrigger(triggerRect: DOMRect): Anchor {
  const margin = 8;
  let left = triggerRect.left;
  left = Math.min(left, window.innerWidth - PANEL_WIDTH - margin);
  left = Math.max(margin, left);
  return {
    left,
    bottom: window.innerHeight - triggerRect.top + PANEL_GAP,
  };
}

export function SidebarSettingsMenu({ onOpenSettings, onLogout, compact = false }: Props) {
  const { t } = useTranslation();
  const triggerRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [present, setPresent] = useState(false);
  const [anchor, setAnchor] = useState<Anchor>({ left: 0, bottom: 0 });
  const [layoutAnimating, setLayoutAnimating] = useState(false);

  const close = () => setOpen(false);

  const syncAnchor = useCallback(() => {
    const triggerRect = triggerRef.current?.getBoundingClientRect();
    if (!triggerRect) return;
    setAnchor(anchorPanelAboveTrigger(triggerRect));
  }, []);

  useLayoutEffect(() => {
    if (!open) {
      setLayoutAnimating(false);
      return;
    }
    syncAnchor();
    setPresent(true);
    setLayoutAnimating(true);
    const fallback = window.setTimeout(() => setLayoutAnimating(false), 820);
    return () => clearTimeout(fallback);
  }, [open, syncAnchor]);

  useEffect(() => {
    if (!open) return;
    const onScrollOrResize = () => syncAnchor();
    window.addEventListener("resize", onScrollOrResize);
    window.addEventListener("scroll", onScrollOrResize, true);
    return () => {
      window.removeEventListener("resize", onScrollOrResize);
      window.removeEventListener("scroll", onScrollOrResize, true);
    };
  }, [open, syncAnchor]);

  useEffect(() => {
    if (!open) return;
    const onOutside = (e: MouseEvent) => {
      const target = e.target as Node;
      if (triggerRef.current?.contains(target) || panelRef.current?.contains(target)) return;
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onOutside);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onOutside);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const run = (action: () => void) => {
    close();
    action();
  };

  const menuContent = (
    <div className={SETTINGS_MENU.pad} onClick={(e) => e.stopPropagation()}>
      <div className={SETTINGS_MENU.account}>
        <CircleUser className="mr-2.5 h-4 w-4 shrink-0 text-text-secondary" />
        <span className="truncate">{t("sidebar.loginApi")}</span>
      </div>
      <div className={MENU_LIST.divider} />
      <SlidingMenuList activeId="__none__" pillClassName={SETTINGS_MENU.pill}>
        <div
          {...{ [MENU_ITEM_ATTR]: "settings" }}
          className={cn(SETTINGS_MENU.row, MOTION.fast)}
          onClick={() => run(onOpenSettings)}
        >
          <Settings className={SETTINGS_MENU.icon} />
          {t("sidebar.settings")}
        </div>
        <div
          {...{ [MENU_ITEM_ATTR]: "logout" }}
          className={cn(SETTINGS_MENU.row, MOTION.fast)}
          onClick={() => run(onLogout)}
        >
          <LogOut className={SETTINGS_MENU.icon} />
          {t("sidebar.logout")}
        </div>
      </SlidingMenuList>
    </div>
  );

  return (
    <div ref={triggerRef} className={cn("relative", !compact && "w-full")}>
      <Button
        variant="ghost"
        aria-label={t("sidebar.settings")}
        aria-expanded={open}
        className={cn(
          compact ? "app-navigation-button" : "flex h-auto w-full justify-start rounded-md px-2.5 py-1.5 text-sm text-text-base",
          MOTION.fast,
          open ? "bg-ui-tint" : "hover:bg-ui-tint",
        )}
        onClick={() => setOpen((v) => !v)}
      >
        <Settings className="h-[18px] w-[18px] shrink-0 text-text-secondary" />
        {!compact && <span className="ml-0.5">{t("sidebar.settings")}</span>}
      </Button>

      {(open || present) &&
        createPortal(
          <div
            style={{
              position: "fixed",
              left: anchor.left,
              bottom: anchor.bottom,
              width: PANEL_WIDTH,
              zIndex: 200,
            }}
          >
            <SpringSurface
              ref={panelRef}
              role="menu"
              data-side="top"
              data-align="start"
              data-state={open ? "open" : "closed"}
              anchor={triggerRef}
              onMotionExit={() => setPresent(false)}
              style={{ transformOrigin: "bottom left" }}
              className={cn(
                "overflow-hidden rounded-2xl bg-elevated-bg shadow-[0_6px_24px_rgba(0,0,0,0.10)]",
                layoutAnimating && "overflow-hidden",
              )}
            >
              <MorphPanelLayoutContext.Provider value={layoutAnimating}>
                {menuContent}
              </MorphPanelLayoutContext.Provider>
            </SpringSurface>
          </div>,
          document.body,
        )}
    </div>
  );
}
