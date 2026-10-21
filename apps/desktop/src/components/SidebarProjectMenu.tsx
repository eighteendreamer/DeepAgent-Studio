import { HoverInfo } from "./ui/HoverInfo";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Archive, FolderOpen, Layers, Network, X } from "lucide-react";
import { PinThumbtackIcon } from "./ui/PinThumbtackIcon";
import { useTranslation } from "react-i18next";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";
import { MENU_LIST, MOTION } from "./ui/motion";
import { SpringSurface } from "./ui/SpringSurface";
import { MorphPanelLayoutContext } from "./ui/MorphPanelLayoutContext";
import { cn } from "./shadcn/utils";

const PANEL_WIDTH = 224;

const PROJECT_MENU = {
  pad: "px-2 py-1.5",
  row: "relative z-[1] flex w-full cursor-pointer items-center rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base whitespace-nowrap",
  icon: "mr-2.5 h-4 w-4 shrink-0 text-[13px] text-text-secondary",
  pill: "left-0 right-0 rounded-lg",
} as const;

type Anchor = { top: number; left: number };

type Props = {
  isPinned: boolean;
  open: boolean;
  contextMenuPosition?: Anchor;
  onOpenChange: (open: boolean) => void;
  onPin: () => void;
  onEdit: () => void;
  onAddSection: () => void;
  onOpenExplorer: () => void;
  onOpenMap: () => void;
  onArchive: () => void;
  onRemove: () => void;
};

const PANEL_GAP = 6;

function anchorPanelRightOfTrigger(triggerRect: DOMRect) {
  const margin = 8;
  let left = triggerRect.right + PANEL_GAP;
  left = Math.min(left, window.innerWidth - PANEL_WIDTH - margin);
  left = Math.max(margin, left);
  return left;
}

export function SidebarProjectMenu({
  isPinned,
  open,
  contextMenuPosition,
  onOpenChange,
  onPin,
  onEdit,
  onAddSection,
  onOpenExplorer,
  onOpenMap,
  onArchive,
  onRemove,
}: Props) {
  const { t } = useTranslation();
  const [present, setPresent] = useState(open);
  const triggerRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const [anchor, setAnchor] = useState<Anchor>({ top: 0, left: 0 });
  const [layoutAnimating, setLayoutAnimating] = useState(false);

  const close = () => onOpenChange(false);

  const syncAnchor = useCallback(() => {
    const triggerRect = triggerRef.current?.getBoundingClientRect();
    if (!triggerRect) return;
    const margin = 8;
    const panelHeight = panelRef.current?.offsetHeight ?? 0;
    setAnchor({
      top: Math.max(margin, Math.min(
        contextMenuPosition?.top ?? triggerRect.top,
        window.innerHeight - panelHeight - margin,
      )),
      left: contextMenuPosition
        ? Math.max(margin, Math.min(contextMenuPosition.left, window.innerWidth - PANEL_WIDTH - margin))
        : anchorPanelRightOfTrigger(triggerRect),
    });
  }, [contextMenuPosition]);

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
      onOpenChange(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onOpenChange(false);
    };
    document.addEventListener("mousedown", onOutside);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onOutside);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onOpenChange]);

  const run = (action: () => void) => {
    close();
    action();
  };

  const menuContent = (
    <div className={PROJECT_MENU.pad} onClick={(e) => e.stopPropagation()}>
      <SlidingMenuList activeId="__none__" pillClassName={PROJECT_MENU.pill}>
        <div
          {...{ [MENU_ITEM_ATTR]: "pin" }}
          className={cn(PROJECT_MENU.row, MOTION.fast)}
          onClick={() => run(onPin)}
        >
          <PinThumbtackIcon pinned={isPinned} className="mr-2.5 h-[13px] w-[13px] shrink-0" />
          {isPinned ? t("sidebar.unpin") : t("sidebar.pin")}
        </div>
        <div
          {...{ [MENU_ITEM_ATTR]: "edit" }}
          className={cn(PROJECT_MENU.row, MOTION.fast)}
          onClick={() => run(onEdit)}
        >
          <FontAwesomeIcon icon={["far", "pen-to-square"]} className={PROJECT_MENU.icon} />
          {t("sidebar.edit")}
        </div>
        <div
          {...{ [MENU_ITEM_ATTR]: "section" }}
          className={cn(PROJECT_MENU.row, MOTION.fast)}
          onClick={() => run(onAddSection)}
        >
          <Layers className={PROJECT_MENU.icon} />
          {t("sidebar.addSection")}
        </div>
        <div
          {...{ [MENU_ITEM_ATTR]: "explorer" }}
          className={cn(PROJECT_MENU.row, MOTION.fast)}
          onClick={() => run(onOpenExplorer)}
        >
          <FolderOpen className={PROJECT_MENU.icon} />
          {t("sidebar.openInExplorer")}
        </div>
        <div
          {...{ [MENU_ITEM_ATTR]: "map" }}
          className={cn(PROJECT_MENU.row, MOTION.fast)}
          onClick={() => run(onOpenMap)}
        >
          <Network className={PROJECT_MENU.icon} />
          项目地图
        </div>
        <div className={MENU_LIST.divider} />
        <div
          {...{ [MENU_ITEM_ATTR]: "archive" }}
          className={cn(PROJECT_MENU.row, MOTION.fast)}
          onClick={() => run(onArchive)}
        >
          <Archive className={PROJECT_MENU.icon} />
          {t("sidebar.archiveChats")}
        </div>
        <div
          {...{ [MENU_ITEM_ATTR]: "remove" }}
          className={cn(PROJECT_MENU.row, MOTION.fast)}
          onClick={() => run(onRemove)}
        >
          <X className={PROJECT_MENU.icon} />
          {t("sidebar.removeProject")}
        </div>
      </SlidingMenuList>
    </div>
  );

  return (
    <div ref={triggerRef} className="relative">
      <HoverInfo content="项目选项"><button
        type="button"
        className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded"

        onClick={(e) => {
          e.stopPropagation();
          onOpenChange(!open);
        }}
      >
        <FontAwesomeIcon icon={["fas", "ellipsis"]} className="text-[10px]" />
      </button></HoverInfo>

      {(open || present) &&
        createPortal(
          <div
            data-custom-contextmenu="1"
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
            }}
            style={{
              position: "fixed",
              top: anchor.top,
              left: anchor.left,
              width: PANEL_WIDTH,
              zIndex: 200,
            }}
          >
            <SpringSurface
              ref={panelRef}
              role="menu"
              data-side={contextMenuPosition ? "bottom" : "right"}
              data-align="start"
              data-state={open ? "open" : "closed"}
              anchor={contextMenuPosition ? undefined : triggerRef}
              onMotionExit={() => setPresent(false)}
              style={{ transformOrigin: "top left" }}
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
