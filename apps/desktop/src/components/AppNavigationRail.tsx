import { Book, Clock, Ellipsis, Layers, PanelLeft, Puzzle, Search, Server, Shapes, SquarePen, type LucideIcon } from "lucide-react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faHouse } from "@fortawesome/free-solid-svg-icons";
import { useTranslation } from "react-i18next";
import { useState } from "react";
import { Button } from "./shadcn/button";
import { cn } from "./shadcn/utils";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./shadcn/dropdown-menu";
import { HoverInfo, HoverInfoGroup } from "./ui/HoverInfo";
import { MenuTriggerOpenContext } from "./ui/MorphPanelLayoutContext";
import { SlidingPill, useSlidingIndicator } from "./ui/SlidingPill";
import { NAVIGATION_MOTION } from "./ui/motion";
import { SidebarSettingsMenu } from "./SidebarSettingsMenu";

type OverflowSurface = "plugins" | "automation" | "canvas" | "remote";
type NavigationSurface = "start" | "chat" | "skills" | "knowledge" | "settings" | OverflowSurface;
const OVERFLOW_NAV: { id: OverflowSurface; icon: LucideIcon }[] = [
  { id: "plugins", icon: Puzzle },
  { id: "automation", icon: Clock },
  { id: "canvas", icon: Shapes },
  { id: "remote", icon: Server },
];

interface Props {
  activeSurface: NavigationSurface;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  onHome: () => void;
  onNewChat: () => void;
  onOpenSearch: () => void;
  onOpenSkills: () => void;
  onOpenKnowledge: () => void;
  onOpenPlugins: () => void;
  onOpenAutomation: () => void;
  onOpenRemote: () => void;
  onOpenCanvas: () => void;
  onOpenSettings: () => void;
  onLogout: () => void;
}

/** Persistent feature navigation; the adjacent sidebar owns projects and chats. */
export function AppNavigationRail(props: Props) {
  const { t } = useTranslation();
  const [otherMenuOpen, setOtherMenuOpen] = useState(false);
  const overflowActive = OVERFLOW_NAV.some(item => item.id === props.activeSurface);
  const activeNavId = otherMenuOpen || overflowActive ? "other" : props.activeSurface;
  const { containerRef, containerProps, indicatorStyle } = useSlidingIndicator({
    hoverSelector: "[data-nav]", activeSelector: `[data-nav="${activeNavId}"]`,
    followFocus: true,
  });
  const overflowActions: Record<OverflowSurface, () => void> = {
    plugins: props.onOpenPlugins, automation: props.onOpenAutomation,
    canvas: props.onOpenCanvas, remote: props.onOpenRemote,
  };
  const mainNav = [
    { id: "new-chat", key: "newChat", icon: SquarePen, action: props.onNewChat },
    { id: "search", key: "search", icon: Search, action: props.onOpenSearch },
    { id: "skills", key: "skills", icon: Layers, action: props.onOpenSkills },
    { id: "knowledge", key: "knowledge", icon: Book, action: props.onOpenKnowledge },
  ];

  return <HoverInfoGroup side="right" delay={NAVIGATION_MOTION.tooltipDelay} closeDelay={0}><nav className="app-navigation-rail" aria-label={t("sidebar.navigation")}>
    <HoverInfo content={t("sidebar.home")} side="right">
    <Button variant="ghost" size="icon" className="app-navigation-home"
      aria-label={t("sidebar.home")}
      aria-current={props.activeSurface === "start" ? "page" : undefined} onClick={props.onHome}>
      <FontAwesomeIcon icon={faHouse} />
    </Button>
    </HoverInfo>
    <div ref={containerRef} {...containerProps} className="app-navigation-items">
      {mainNav.map(item => <HoverInfo key={item.id} content={t(`sidebar.${item.key}`)} side="right">
        <Button variant="ghost" size="icon" className="app-navigation-button relative z-[1] hover:bg-transparent"
          data-nav={item.id} aria-label={t(`sidebar.${item.key}`)}
          aria-current={activeNavId === item.id ? "page" : undefined} onClick={item.action}>
          <item.icon />
        </Button>
      </HoverInfo>)}
      <DropdownMenu modal={false} onOpenChange={setOtherMenuOpen}>
        <MenuTriggerOpenContext.Provider value={otherMenuOpen}>
        <HoverInfo content={t("sidebar.other")} side="right">
        <DropdownMenuTrigger asChild>
          <Button variant="ghost" size="icon" data-nav="other" className="app-navigation-button relative z-[1] hover:bg-transparent"
            aria-label={t("sidebar.other")} aria-current={overflowActive ? "page" : undefined}>
            <Ellipsis />
          </Button>
        </DropdownMenuTrigger>
        </HoverInfo>
        </MenuTriggerOpenContext.Provider>
        <DropdownMenuContent side="right" align="start" sideOffset={10} className="min-w-[10.5rem]">
          {OVERFLOW_NAV.map(item => <DropdownMenuItem key={item.id}
            className={cn("gap-2 text-sm", props.activeSurface === item.id && "font-medium bg-sidebar-highlight")}
            onSelect={() => overflowActions[item.id]()}>
            <item.icon className="h-4 w-4 shrink-0 text-text-secondary" />{t(`sidebar.${item.id}`)}
          </DropdownMenuItem>)}
        </DropdownMenuContent>
      </DropdownMenu>
      <SlidingPill style={indicatorStyle} motion={NAVIGATION_MOTION} className="left-0 right-0 bg-sidebar-highlight rounded-[10px]" />
    </div>
    <div className="app-navigation-bottom">
      <HoverInfo content={t(props.sidebarOpen ? "sidebar.collapseSidebar" : "sidebar.expandSidebar")} side="right">
        <Button variant="ghost" size="icon" className="app-navigation-button" data-sidebar-toggle="rail"
          aria-label={t(props.sidebarOpen ? "sidebar.collapseSidebar" : "sidebar.expandSidebar")}
          aria-expanded={props.sidebarOpen} aria-controls="app-context-sidebar" onClick={props.onToggleSidebar}>
          <PanelLeft />
        </Button>
      </HoverInfo>
      <SidebarSettingsMenu compact onOpenSettings={props.onOpenSettings} onLogout={props.onLogout} />
    </div>
  </nav></HoverInfoGroup>;
}
