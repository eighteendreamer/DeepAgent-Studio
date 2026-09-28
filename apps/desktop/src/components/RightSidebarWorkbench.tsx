import { HoverInfo } from "./ui/HoverInfo";
import { useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { SidebarPluginHeader } from "./SidebarPluginHeader";
import { ToolLauncherPanel, type ToolLauncherCard } from "./ToolLauncherPanel";
import {
  createPluginTab,
  getPluginDefinition,
  renderPluginTab,
  type PluginRenderContext,
  type PluginTab,
  type PluginTitleContext,
  type PluginToolCard,
  type PluginType,
} from "./plugins/pluginRegistry";
import { usePluginAppCards } from "./plugins/usePluginAppCards";
import { SIDEBAR_MIN_WIDTH, useResizableSidebar } from "../hooks/useResizableSidebar";
import { usePanelPresence } from "../hooks/usePanelPresence";
import { openStudioCanvasWindow } from "../api";
import { message } from "./message";

const STUDIO_CANVAS_CARD: ToolLauncherCard = {
  type: "canvas",
  icon: ["fas", "border-all"],
  title: "Studio Canvas",
  desc: "Open extensible workspace",
};

interface RightSidebarWorkbenchProps {
  open: boolean;
  tabs: PluginTab[];
  activeTabId: string;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onShowLauncher: () => void;
  onSelectPlugin: (card: PluginToolCard) => void;
  renderContext?: PluginRenderContext;
  extraActions?: React.ReactNode;
}

const SIDEBAR_ANIM_MS = 400;

export function RightSidebarWorkbench({
  open,
  tabs,
  activeTabId,
  onSelectTab,
  onCloseTab,
  onShowLauncher,
  onSelectPlugin,
  renderContext,
  extraActions,
}: RightSidebarWorkbenchProps) {
  const [pluginAppCards] = usePluginAppCards(open);
  const presence = usePanelPresence(open, SIDEBAR_ANIM_MS);
  const [shellWidth, setShellWidth] = useState(0);

  const visibleTabs = useMemo(
    () => tabs.filter((tab) => getPluginDefinition(tab.type) != null),
    [tabs],
  );
  const activeTab = visibleTabs.find((tab) => tab.id === activeTabId) ?? null;
  const showPluginContent = Boolean(activeTab && activeTabId !== "new");

  const sidebarMinWidth = useMemo(() => {
    if (!showPluginContent || !activeTab) return SIDEBAR_MIN_WIDTH.launcher;
    switch (activeTab.type) {
      case "chat":
        return SIDEBAR_MIN_WIDTH.chat;
      case "files":
        return SIDEBAR_MIN_WIDTH.files;
      case "browser":
        return SIDEBAR_MIN_WIDTH.browser;
      case "terminal":
        return SIDEBAR_MIN_WIDTH.terminal;
      default:
        return SIDEBAR_MIN_WIDTH.default;
    }
  }, [activeTab, showPluginContent]);

  const { width, sidebarRef, isResizing, startResizing, isMaximized, toggleMaximize, resetMaximize } =
    useResizableSidebar({ defaultWidth: 400, minWidth: sidebarMinWidth });
  const availablePluginCards = pluginAppCards;
  const launcherCards: ToolLauncherCard[] = useMemo(
    () => [...availablePluginCards, STUDIO_CANVAS_CARD],
    [availablePluginCards],
  );

  const showHeader = visibleTabs.length > 0;

  const targetShellWidth = showPluginContent ? Math.max(width, sidebarMinWidth) : width;

  useEffect(() => {
    if (tabs.length === 0) resetMaximize();
  }, [resetMaximize, tabs.length]);

  useEffect(() => {
    if (!presence.shouldRender) {
      setShellWidth(0);
      return;
    }

    if (!open) {
      setShellWidth(0);
      return;
    }

    if (presence.phase === "opening") {
      setShellWidth(0);
      const id = window.requestAnimationFrame(() => setShellWidth(targetShellWidth));
      return () => window.cancelAnimationFrame(id);
    }

    setShellWidth(targetShellWidth);
  }, [open, presence.shouldRender, presence.phase, targetShellWidth]);

  const handleLauncherSelect = (card: ToolLauncherCard) => {
    if (card.type === STUDIO_CANVAS_CARD.type) {
      void openStudioCanvasWindow().catch((error) => {
        message.error(`打开工作画布失败：${String(error)}`);
      });
      return;
    }
    const existing = visibleTabs.find((tab) => tab.type === card.type);
    if (existing) {
      onSelectTab(existing.id);
      return;
    }
    onSelectPlugin(card as PluginToolCard);
  };

  if (!presence.shouldRender) return null;

  const headerExtraActions =
    activeTab?.type === "files" || extraActions ? (
      <>
        {activeTab?.type === "files" ? (
          <HoverInfo content={isMaximized ? "退出全屏文件视图" : "全屏文件视图"}><button
            type="button"
            onClick={toggleMaximize}
            className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-hover-bg hover:text-text-base"

          >
            <FontAwesomeIcon
              icon={["fas", isMaximized ? "compress" : "expand"]}
              className="text-[12px]"
            />
          </button></HoverInfo>
        ) : null}
        {extraActions}
      </>
    ) : null;

  if (isMaximized) {
    return (
      <aside
        ref={sidebarRef}
        className="right-sidebar-workbench absolute inset-0 z-40 h-full overflow-hidden is-maximized"
      >
        <div className="relative flex h-full w-full flex-col overflow-hidden bg-bg-base">
          <SidebarPluginHeader
            tabs={visibleTabs}
            activeTabId={activeTabId}
            onSelectTab={onSelectTab}
            onCloseTab={onCloseTab}
            onShowLauncher={onShowLauncher}
            availablePlugins={availablePluginCards}
            onSelectPlugin={(plugin) => onSelectPlugin(plugin as PluginToolCard & { type: PluginType })}
            reserveRightActionsSpace
            extraActions={headerExtraActions}
          />
          <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
            {activeTab ? renderPluginTab(activeTab, renderContext) : null}
          </div>
        </div>
      </aside>
    );
  }

  const totalWidth = presence.shouldRender ? shellWidth : 0;
  const isOpening = presence.phase === "opening";

  return (
    <aside
      ref={sidebarRef}
      className={`right-sidebar-workbench relative flex h-full flex-shrink-0 overflow-hidden ${
        isOpening ? "is-opening" : ""
      } ${presence.isClosing ? "is-closing" : ""} ${isResizing ? "is-resizing" : ""}`}
      style={{ width: totalWidth }}
    >
      <div className="relative flex h-full w-full flex-col overflow-hidden bg-bg-base">
        <div
          className={`panel-resize-handle-col ${isResizing ? "is-active" : ""}`}
          onMouseDown={(e) => {
            e.preventDefault();
            startResizing();
          }}
        />

        {showHeader && (
          <SidebarPluginHeader
            tabs={visibleTabs}
            activeTabId={activeTabId}
            onSelectTab={onSelectTab}
            onCloseTab={onCloseTab}
            onShowLauncher={onShowLauncher}
            availablePlugins={availablePluginCards}
            onSelectPlugin={(plugin) => onSelectPlugin(plugin as PluginToolCard & { type: PluginType })}
            reserveRightActionsSpace
            extraActions={headerExtraActions}
          />
        )}

        <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
          {showPluginContent && activeTab ? (
            renderPluginTab(activeTab, renderContext)
          ) : (
            <ToolLauncherPanel cards={launcherCards} onSelect={handleLauncherSelect} variant="codex" />
          )}
        </div>
      </div>
    </aside>
  );
}

export { createPluginTab };
export type { PluginTab, PluginTitleContext, PluginToolCard };
