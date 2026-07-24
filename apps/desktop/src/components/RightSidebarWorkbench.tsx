import { useCallback, useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { SidebarPluginHeader } from "./SidebarPluginHeader";
import { ToolLauncherPanel } from "./ToolLauncherPanel";
import type { ToolLauncherCard } from "./ToolLauncherPanel";
import {
  createPluginTab,
  getPluginDefinition,
  pluginAppToToolCard,
  PLUGIN_TOOL_CARDS,
  renderPluginTab,
  type PluginRenderContext,
  type PluginTab,
  type PluginTitleContext,
  type PluginToolCard,
  type PluginType,
} from "./plugins/pluginRegistry";
import { useResizableSidebar } from "../hooks/useResizableSidebar";
import { listPluginApps, openStudioCanvasWindow, PLUGINS_CHANGED_EVENT } from "../api";
import { message } from "./message";

const STUDIO_CANVAS_CARD: ToolLauncherCard = {
  type: "canvas",
  icon: ["fas", "border-all"],
  title: "Studio Canvas",
  desc: "Open extensible workspace",
};

const STATIC_PLUGIN_TYPES = new Set(PLUGIN_TOOL_CARDS.map((card) => card.type));

interface RightSidebarWorkbenchProps {
  open: boolean;
  /** Tabs currently open in the sidebar (owned by the parent view). */
  tabs: PluginTab[];
  /** Id of the active tab, or "new" to show the plugin launcher. */
  activeTabId: string;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onShowLauncher: () => void;
  onSelectPlugin: (card: PluginToolCard) => void;
  renderContext?: PluginRenderContext;
  /** Extra actions rendered next to the launcher/close buttons in the tab header. */
  extraActions?: React.ReactNode;
}

/**
 * Unified right-side workbench shared by `ChatView` and `StartView`. Hosts
 * the plugin tab strip (files / chat / browser / terminal / project map /
 * recording / file preview), a drag-to-resize handle, and a "maximize" mode
 * for the files plugin. Width is always clamped so the conversation column
 * keeps at least `MIN_CHAT_WIDTH` (500px) - see `useResizableSidebar`.
 */
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
  const { width, sidebarRef, isResizing, startResizing, isMaximized, toggleMaximize, resetMaximize } =
    useResizableSidebar();
  const [pluginAppCards, setPluginAppCards] = useState<PluginToolCard[]>([]);

  const visibleTabs = useMemo(
    () => tabs.filter((tab) => getPluginDefinition(tab.type) != null),
    [tabs],
  );
  const activeTab = visibleTabs.find((tab) => tab.id === activeTabId) ?? null;
  const visiblePluginAppCards = useMemo(
    () =>
      pluginAppCards.filter(
        (card) =>
          !(card.pluginId?.endsWith("@builtin") && STATIC_PLUGIN_TYPES.has(card.type)),
      ),
    [pluginAppCards],
  );
  const availablePluginCards = useMemo(
    () => [...PLUGIN_TOOL_CARDS, ...visiblePluginAppCards],
    [visiblePluginAppCards],
  );
  const launcherCards: ToolLauncherCard[] = [...availablePluginCards, STUDIO_CANVAS_CARD];

  const handleLauncherSelect = (card: ToolLauncherCard) => {
    if (card.type === STUDIO_CANVAS_CARD.type) {
      void openStudioCanvasWindow().catch((error) => {
        message.error(`打开工作画布失败：${String(error)}`);
      });
      return;
    }
    onSelectPlugin(card as PluginToolCard);
  };

  // Dropping back to zero open tabs (all closed) should also drop out of
  // maximize mode so the next open starts at the default width.
  useEffect(() => {
    if (tabs.length === 0) resetMaximize();
  }, [resetMaximize, tabs.length]);

  const refreshPluginApps = useCallback(async () => {
    if (!open) return;
    try {
      const apps = await listPluginApps();
      const cards = apps
        .map(pluginAppToToolCard)
        .filter((card): card is PluginToolCard => Boolean(card));
      setPluginAppCards(cards);
    } catch (error) {
      console.warn("failed to load plugin apps", error);
      setPluginAppCards([]);
    }
  }, [open]);

  useEffect(() => {
    void refreshPluginApps();
  }, [refreshPluginApps]);

  useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = () => {
      void refreshPluginApps();
    };
    window.addEventListener(PLUGINS_CHANGED_EVENT, handler);
    return () => {
      window.removeEventListener(PLUGINS_CHANGED_EVENT, handler);
    };
  }, [refreshPluginApps]);

  const maximizedClasses = "fixed top-10 left-0 right-0 bottom-0 z-[60] border-l-0 shadow-none";
  const normalClasses = "relative z-10 flex-shrink-0";

  if (!open) return null;

  return (
    <aside
      ref={sidebarRef}
      className={`right-sidebar-workbench h-full overflow-hidden ${isMaximized ? maximizedClasses : normalClasses}`}
      style={isMaximized ? { width: "100%" } : { width }}
    >
      <div
        className="flex h-full flex-col overflow-hidden border-l border-border-theme bg-white shadow-[-12px_0_30px_rgba(15,23,42,0.06)]"
        style={isMaximized ? { width: "100%" } : { width, minWidth: 360 }}
      >
      {/* Drag handle on the left edge (hidden in maximize mode). */}
      {!isMaximized && (
      <div
        className={`panel-resize-handle-col ${isResizing ? "is-active" : ""}`}
        onMouseDown={(e) => {
          e.preventDefault();
          startResizing();
        }}
      />
      )}

      <SidebarPluginHeader
        tabs={visibleTabs}
        activeTabId={activeTabId}
        onSelectTab={onSelectTab}
        onCloseTab={onCloseTab}
        onShowLauncher={onShowLauncher}
        availablePlugins={availablePluginCards}
        onSelectPlugin={(plugin) =>
          onSelectPlugin(plugin as PluginToolCard & { type: PluginType })
        }
        reserveRightActionsSpace
        extraActions={
          <>
            {activeTab?.type === "files" ? (
              <button
                type="button"
                onClick={toggleMaximize}
                className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-[#f3f4f6] hover:text-text-base"
                title={isMaximized ? "退出全屏文件视图" : "全屏文件视图"}
              >
                <FontAwesomeIcon
                  icon={["fas", isMaximized ? "compress" : "expand"]}
                  className="text-[12px]"
                />
              </button>
            ) : null}
            {extraActions}
          </>
        }
      />

      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {activeTabId === "new" || !activeTab ? (
          <ToolLauncherPanel
            cards={launcherCards}
            onSelect={handleLauncherSelect}
            variant="sidebar"
          />
        ) : (
          renderPluginTab(activeTab, renderContext)
        )}
      </div>
      </div>
    </aside>
  );
}

/** Re-exported so parent views can build tab objects without importing the registry directly. */
export { createPluginTab };
export type { PluginTab, PluginTitleContext, PluginToolCard };
