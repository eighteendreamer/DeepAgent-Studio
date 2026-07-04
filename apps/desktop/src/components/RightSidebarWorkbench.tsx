import { useEffect } from "react";
import { AnimatePresence, motion } from "framer-motion";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { SidebarPluginHeader } from "./SidebarPluginHeader";
import { ToolLauncherPanel } from "./ToolLauncherPanel";
import {
  createPluginTab,
  PLUGIN_TOOL_CARDS,
  renderPluginTab,
  type PluginRenderContext,
  type PluginTab,
  type PluginTitleContext,
  type PluginToolCard,
  type PluginType,
} from "./plugins/pluginRegistry";
import { useResizableSidebar } from "../hooks/useResizableSidebar";

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

  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? null;

  // Dropping back to zero open tabs (all closed) should also drop out of
  // maximize mode so the next open starts at the default width.
  useEffect(() => {
    if (tabs.length === 0) resetMaximize();
  }, [resetMaximize, tabs.length]);

  const maximizedClasses = "fixed top-10 left-0 right-0 bottom-0 z-[60] border-l-0 shadow-none";
  const normalClasses = "relative z-10 flex-shrink-0";

  return (
    <AnimatePresence>
      {open && (
    <motion.aside
      ref={sidebarRef}
      key="right-sidebar-workbench"
      initial={{ width: 0, opacity: 0, x: 20 }}
      animate={{ width: isMaximized ? "100%" : width, opacity: 1, x: 0 }}
      exit={{ width: 0, opacity: 0, x: 20 }}
      transition={isResizing ? { duration: 0 } : { type: "spring", bounce: 0, duration: 0.3 }}
      className={`h-full overflow-hidden ${isMaximized ? maximizedClasses : normalClasses}`}
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
        tabs={tabs}
        activeTabId={activeTabId}
        onSelectTab={onSelectTab}
        onCloseTab={onCloseTab}
        onShowLauncher={onShowLauncher}
        availablePlugins={PLUGIN_TOOL_CARDS}
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
            cards={PLUGIN_TOOL_CARDS}
            onSelect={(card) => onSelectPlugin(card)}
            variant="sidebar"
          />
        ) : (
          renderPluginTab(activeTab, renderContext)
        )}
      </div>
      </div>
    </motion.aside>
      )}
    </AnimatePresence>
  );
}

/** Re-exported so parent views can build tab objects without importing the registry directly. */
export { createPluginTab };
export type { PluginTab, PluginTitleContext, PluginToolCard };
