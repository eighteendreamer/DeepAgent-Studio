import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { motion, useReducedMotion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";
import { MENU_LIST, MOTION } from "./ui/motion";
import { morphSpringTransition } from "./ui/morphingMenuMotion";
import { MorphPanelLayoutContext } from "./ui/MorphPanelLayoutContext";
import { cn } from "./shadcn/utils";

const PANEL_WIDTH = 224;
const PANEL_GAP = 6;

const SETTINGS_MENU = {
  pad: "px-2 py-1.5",
  row: "relative z-[1] flex w-full cursor-pointer items-center rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base whitespace-nowrap hover:bg-transparent",
  icon: "mr-2.5 w-4 shrink-0 text-[13px] text-text-secondary",
  pill: "left-0 right-0 rounded-lg",
  account: "flex min-w-0 items-center px-2.5 py-2 text-[13px] font-medium text-text-base",
} as const;

type Anchor = { left: number; bottom: number };

type Props = {
  onOpenSettings: () => void;
  onLogout: () => void;
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

export function SidebarSettingsMenu({ onOpenSettings, onLogout }: Props) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const spring = morphSpringTransition(reduced);
  const triggerRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
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
    setLayoutAnimating(true);
    const fallback = window.setTimeout(() => setLayoutAnimating(false), 420);
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
        <FontAwesomeIcon icon={["fas", "circle-user"]} className="mr-2.5 shrink-0 text-base text-text-secondary" />
        <span className="truncate">{t("sidebar.loginApi")}</span>
      </div>
      <div className={MENU_LIST.divider} />
      <SlidingMenuList activeId="__none__" pillClassName={SETTINGS_MENU.pill}>
        <div
          {...{ [MENU_ITEM_ATTR]: "settings" }}
          className={cn(SETTINGS_MENU.row, MOTION.fast)}
          onClick={() => run(onOpenSettings)}
        >
          <FontAwesomeIcon icon={["fas", "gear"]} className={SETTINGS_MENU.icon} />
          {t("sidebar.settings")}
        </div>
        <div
          {...{ [MENU_ITEM_ATTR]: "logout" }}
          className={cn(SETTINGS_MENU.row, MOTION.fast)}
          onClick={() => run(onLogout)}
        >
          <FontAwesomeIcon icon={["fas", "arrow-right-from-bracket"]} className={SETTINGS_MENU.icon} />
          {t("sidebar.logout")}
        </div>
      </SlidingMenuList>
    </div>
  );

  return (
    <div ref={triggerRef} className="relative w-full">
      <button
        type="button"
        className={cn(
          "flex w-full items-center rounded-md px-2.5 py-1.5 text-sm text-text-base",
          MOTION.fast,
          open ? "bg-black/5" : "hover:bg-black/5",
        )}
        onClick={() => setOpen((v) => !v)}
      >
        <FontAwesomeIcon icon={["fas", "gear"]} className="w-5 text-left text-text-secondary" />
        <span className="ml-0.5">{t("sidebar.settings")}</span>
      </button>

      {open &&
        createPortal(
          <div
            style={{
              position: "fixed",
              left: anchor.left,
              bottom: anchor.bottom,
              width: PANEL_WIDTH,
              zIndex: 200,
              filter: "drop-shadow(0 6px 24px rgba(0,0,0,0.10))",
            }}
          >
            <motion.div
              ref={panelRef}
              role="menu"
              initial={reduced ? false : { opacity: 0.6, scale: 0.15 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.15 }}
              transition={spring}
              style={{ transformOrigin: "bottom left" }}
              onAnimationComplete={() => {
                if (open) setLayoutAnimating(false);
              }}
              className={cn(
                "overflow-hidden rounded-2xl bg-elevated-bg",
                layoutAnimating && "overflow-hidden",
              )}
            >
              <MorphPanelLayoutContext.Provider value={layoutAnimating}>
                {menuContent}
              </MorphPanelLayoutContext.Provider>
            </motion.div>
          </div>,
          document.body,
        )}
    </div>
  );
}
