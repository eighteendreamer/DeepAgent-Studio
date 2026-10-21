import { HoverInfo } from "./ui/HoverInfo";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef } from "react";
import gsap from "gsap";
import { SOFT_SPRING, prefersReducedMotion, springEase } from "./ui/springMotion";
import { Maximize2, Minimize2 } from "lucide-react";
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
  const shellRef = useRef<HTMLElement | null>(null);

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
  const setShellRef = useCallback((node: HTMLElement | null) => {
    shellRef.current = node;
    sidebarRef(node);
  }, [sidebarRef]);
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

  useLayoutEffect(() => {
    const shell = shellRef.current;
    if (!shell || !presence.shouldRender) return;
    shell.inert = !open;
    const content = shell.firstElementChild;
    if (isMaximized) {
      gsap.set(shell, { clearProps: "width" });
      const fade = content ? gsap.to(content, { opacity: open ? 1 : 0,
        duration: prefersReducedMotion() ? 0 : 0.18 }) : null;
      return () => { fade?.kill(); };
    }
    const instant = isResizing || prefersReducedMotion();
    const timeline = gsap.timeline();
    timeline.to(shell, { width: open ? targetShellWidth : 0,
      duration: instant ? 0 : 0.36,
      ease: springEase(SOFT_SPRING.stiffness, SOFT_SPRING.damping, 0.36) }, 0);
    if (content) timeline.to(content, { opacity: open ? 1 : 0,
      duration: instant ? 0 : open ? 0.24 : 0.14, ease: "power2.out" }, 0);
    return () => { timeline.kill(); };
  }, [open, presence.shouldRender, targetShellWidth, isResizing, isMaximized]);

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
            {isMaximized ? (
              <Minimize2 className="h-3.5 w-3.5" strokeWidth={1.8} aria-hidden="true" />
            ) : (
              <Maximize2 className="h-3.5 w-3.5" strokeWidth={1.8} aria-hidden="true" />
            )}
          </button></HoverInfo>
        ) : null}
        {extraActions}
      </>
    ) : null;

  if (isMaximized) {
    return (
      <aside
        ref={setShellRef}
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

  const isOpening = presence.phase === "opening";

  return (
    <aside
      ref={setShellRef}
      data-spring-panel
      className={`right-sidebar-workbench relative flex h-full flex-shrink-0 overflow-hidden ${
        isOpening ? "is-opening" : ""
      } ${presence.isClosing ? "is-closing" : ""} ${isResizing ? "is-resizing" : ""}`}
      style={{ width: 0 }}
    >
      <div className="relative flex h-full shrink-0 flex-col overflow-hidden bg-bg-base" style={{ width: targetShellWidth }}>
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
