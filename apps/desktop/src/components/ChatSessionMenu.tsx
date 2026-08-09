import { useId, useLayoutEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { TimelineEntry } from "../types";
import { MorphingMenuShell } from "./ui/MorphingMenuShell";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";
import { MENU_LIST, MOTION } from "./ui/motion";
import { Panel } from "./ui/Panel";
import { cn } from "./shadcn/utils";

const SESSION_MENU = {
  pad: "px-2 py-1.5",
  row: "relative z-[1] flex w-full cursor-pointer items-center rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base whitespace-nowrap",
  icon: "mr-2.5 w-4 shrink-0 text-[13px] text-text-secondary",
  shortcut: "ml-auto shrink-0 pl-3 text-[10px] tabular-nums text-text-secondary",
  pill: "left-0 right-0 rounded-lg",
} as const;

type Props = {
  title?: string | null;
  pinned?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  rewindEntries: TimelineEntry[];
  formatRewindTimestamp: (timestamp: number) => string;
  onPin?: () => void;
  onRename?: () => void;
  onArchive?: () => void;
  onCopy?: () => void;
  onExport?: () => void;
  onFork?: () => void;
  onRewindEntry?: (sequence: number, detail?: string) => void;
  onOpenAutomation?: () => void;
  onOpenInNewWindow?: () => void;
};

export function ChatSessionMenu({
  title,
  pinned,
  open,
  onOpenChange,
  rewindEntries,
  formatRewindTimestamp,
  onPin,
  onRename,
  onArchive,
  onCopy,
  onExport,
  onFork,
  onRewindEntry,
  onOpenAutomation,
  onOpenInNewWindow,
}: Props) {
  const { t } = useTranslation();
  const layoutId = useId().replace(/:/g, "");
  const padRef = useRef<HTMLDivElement>(null);
  const rewindRowRef = useRef<HTMLDivElement>(null);
  const [isRewindFlyoutOpen, setIsRewindFlyoutOpen] = useState(false);
  const [rewindFlyoutAnchor, setRewindFlyoutAnchor] = useState({ top: 0, height: 36 });

  useLayoutEffect(() => {
    if (!isRewindFlyoutOpen || !rewindRowRef.current || !padRef.current) return;
    const rowRect = rewindRowRef.current.getBoundingClientRect();
    const padRect = padRef.current.getBoundingClientRect();
    setRewindFlyoutAnchor({
      top: rowRect.top - padRect.top,
      height: rowRect.height,
    });
  }, [isRewindFlyoutOpen, open]);

  const close = () => {
    setIsRewindFlyoutOpen(false);
    onOpenChange(false);
  };

  const run = (action?: () => void) => {
    close();
    action?.();
  };

  const trigger = (
    <button
      type="button"
      className={cn(
        "flex h-8 max-w-full min-w-0 cursor-pointer items-center gap-2 rounded-md pl-1 pr-2 text-sm font-medium text-text-base outline-none",
        MOTION.fast,
        open ? "bg-black/5" : "hover:bg-black/5",
      )}
      onClick={() => onOpenChange(!open)}
    >
      <span className="truncate">{title?.trim() || t("chatView.chat")}</span>
      <FontAwesomeIcon icon={["fas", "ellipsis"]} className="shrink-0 text-[10px] text-text-secondary" />
    </button>
  );

  return (
    <MorphingMenuShell
      open={open}
      onOpenChange={(next) => {
        if (!next) setIsRewindFlyoutOpen(false);
        onOpenChange(next);
      }}
      layoutId={layoutId}
      trigger={trigger}
      panelPlacement="below"
      panelAlign="left"
      panelClassName="mt-1.5 w-[min(280px,calc(100vw-48px))] overflow-visible"
      staggerContent={false}
      zIndex={50}
    >
      <div ref={padRef} className={cn(SESSION_MENU.pad, "relative overflow-visible")}>
        <SlidingMenuList activeId="__none__" pillClassName={SESSION_MENU.pill}>
          <div
            {...{ [MENU_ITEM_ATTR]: "pin" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onPin)}
          >
            <FontAwesomeIcon icon={["fas", "thumbtack"]} className={SESSION_MENU.icon} />
            <span>{pinned ? t("sidebar.unpin") : t("chatView.pinChat")}</span>
            <span className={SESSION_MENU.shortcut}>Ctrl+Alt+P</span>
          </div>
          <div
            {...{ [MENU_ITEM_ATTR]: "rename" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onRename)}
          >
            <FontAwesomeIcon icon={["fas", "pen"]} className={SESSION_MENU.icon} />
            <span>{t("chatView.renameChat")}</span>
            <span className={SESSION_MENU.shortcut}>Ctrl+Alt+R</span>
          </div>
          <div
            {...{ [MENU_ITEM_ATTR]: "archive" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onArchive)}
          >
            <FontAwesomeIcon icon={["fas", "box-archive"]} className={SESSION_MENU.icon} />
            <span>{t("chatView.archiveChat")}</span>
            <span className={SESSION_MENU.shortcut}>Ctrl+Shift+A</span>
          </div>

          <div className={MENU_LIST.divider} />

          <div
            {...{ [MENU_ITEM_ATTR]: "copy" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onCopy)}
          >
            <FontAwesomeIcon icon={["far", "copy"]} className={SESSION_MENU.icon} />
            <span>{t("chatView.copy")}</span>
          </div>
          <div
            {...{ [MENU_ITEM_ATTR]: "export" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onExport)}
          >
            <FontAwesomeIcon icon={["fas", "file-export"]} className={SESSION_MENU.icon} />
            <span>{t("chatView.exportJson")}</span>
          </div>
          <div
            {...{ [MENU_ITEM_ATTR]: "branch" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onFork)}
          >
            <FontAwesomeIcon icon={["fas", "code-branch"]} className={SESSION_MENU.icon} />
            <span>{t("chatView.branch")}</span>
          </div>

          <div
            ref={rewindRowRef}
            {...{ [MENU_ITEM_ATTR]: "rewind" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onMouseEnter={() => setIsRewindFlyoutOpen(true)}
          >
            <FontAwesomeIcon icon={["fas", "clock-rotate-left"]} className={SESSION_MENU.icon} />
            <span className="min-w-0 flex-1 truncate">{t("chatView.rewind")}</span>
            <FontAwesomeIcon icon={["fas", "chevron-right"]} className="ml-2 shrink-0 text-[10px] text-text-secondary" />
          </div>

          <div className={MENU_LIST.divider} />

          <div
            {...{ [MENU_ITEM_ATTR]: "automation" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onOpenAutomation)}
          >
            <FontAwesomeIcon icon={["far", "clock"]} className={SESSION_MENU.icon} />
            <span>{t("chatView.addAutomation")}</span>
          </div>

          <div className={MENU_LIST.divider} />

          <div
            {...{ [MENU_ITEM_ATTR]: "new-window" }}
            className={cn(SESSION_MENU.row, MOTION.fast, "hover:bg-transparent")}
            onClick={() => run(onOpenInNewWindow)}
          >
            <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className={SESSION_MENU.icon} />
            <span>{t("chatView.openInNewWindow")}</span>
          </div>
        </SlidingMenuList>

        {isRewindFlyoutOpen && (
          <>
            <div
              className="absolute left-full z-[65] w-2"
              style={{ top: rewindFlyoutAnchor.top, height: rewindFlyoutAnchor.height }}
              onMouseEnter={() => setIsRewindFlyoutOpen(true)}
              aria-hidden
            />
            <Panel
              menu
              className="absolute left-full top-0 z-[70] ml-1.5 max-h-72 w-[min(280px,calc(100vw-48px))] origin-top-left overflow-y-auto shadow-none"
              style={{ top: rewindFlyoutAnchor.top }}
              onMouseEnter={() => setIsRewindFlyoutOpen(true)}
              onMouseLeave={() => setIsRewindFlyoutOpen(false)}
            >
              <div className="px-2 py-1.5">
                {rewindEntries.length === 0 ? (
                  <div className="px-2.5 py-2 text-[12px] text-text-secondary">{t("chatView.noRewindPoints")}</div>
                ) : (
                  <SlidingMenuList activeId="__none__" pillClassName={SESSION_MENU.pill}>
                    {rewindEntries.map((entry) => (
                      <div
                        key={entry.sequence}
                        {...{ [MENU_ITEM_ATTR]: `rewind-${entry.sequence}` }}
                        className={cn(
                          "relative z-[1] flex w-full cursor-pointer items-start gap-2 rounded-lg px-2.5 py-2.5 text-left hover:bg-transparent",
                          MOTION.fast,
                        )}
                        onClick={() => {
                          const detail = typeof entry.detail === "string" ? entry.detail : undefined;
                          run(() => onRewindEntry?.(Math.max(0, entry.sequence - 1), detail));
                        }}
                      >
                        <span className="shrink-0 tabular-nums text-[11px] text-text-secondary">#{entry.sequence}</span>
                        <div className="min-w-0 flex-1">
                          <div className="truncate text-[13px] text-text-base">{entry.detail}</div>
                          <div className="mt-0.5 text-[11px] text-text-secondary">
                            {formatRewindTimestamp(entry.timestamp)}
                          </div>
                        </div>
                      </div>
                    ))}
                  </SlidingMenuList>
                )}
              </div>
            </Panel>
          </>
        )}
      </div>
    </MorphingMenuShell>
  );
}
