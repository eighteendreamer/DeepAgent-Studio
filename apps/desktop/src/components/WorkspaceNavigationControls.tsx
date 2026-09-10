import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { SidebarLeftIcon } from "./icons";
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

export function WorkspaceNavigationControls({
  onToggleSidebar,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
  className,
}: Props) {
  const { t } = useTranslation();
  const buttonClass =
    "flex h-7 w-7 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-sidebar-highlight hover:text-text-base disabled:cursor-default disabled:opacity-35 disabled:hover:bg-transparent";

  return (
    <div className={cn("flex items-center gap-1", className)}>
      <Button
        variant="ghost"
        size="icon"
        className={buttonClass}
        onClick={onToggleSidebar}
        title={t("settings.shortcuts.toggleSidebar")}
        aria-label={t("settings.shortcuts.toggleSidebar")}
      >
        <SidebarLeftIcon />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className={buttonClass}
        onClick={onBack}
        disabled={!canGoBack}
        title={t("settings.shortcuts.goBack")}
        aria-label={t("settings.shortcuts.goBack")}
      >
        <FontAwesomeIcon icon={["fas", "arrow-left"]} className="text-[11px]" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className={buttonClass}
        onClick={onForward}
        disabled={!canGoForward}
        title={t("settings.shortcuts.goForward")}
        aria-label={t("settings.shortcuts.goForward")}
      >
        <FontAwesomeIcon icon={["fas", "arrow-right"]} className="text-[11px]" />
      </Button>
    </div>
  );
}
