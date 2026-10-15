import { BottomPanelIcon, SidebarRightIcon } from "./icons";
import { Button } from "./shadcn/button";
import { HoverInfo } from "./ui/HoverInfo";

interface Props {
  isRightSidebarOpen: boolean;
  onToggleRightSidebar: () => void;
  isBottomPanelOpen: boolean;
  onToggleBottomPanel: () => void;
}

/** Keep window actions on the same 32px row as the workbench tabs. */
export function WorkbenchWindowActions({
  isRightSidebarOpen,
  onToggleRightSidebar,
  isBottomPanelOpen,
  onToggleBottomPanel,
}: Props) {
  const sidebarLabel = isRightSidebarOpen ? "收起侧栏" : "打开右侧栏";
  return (
    <div className="pointer-events-auto absolute right-6 top-0 z-50 flex h-8 items-center gap-3 text-text-secondary">
      <HoverInfo content={sidebarLabel}>
        <Button
          variant="ghost"
          size="icon"
          onClick={onToggleRightSidebar}
          aria-label={sidebarLabel}
          className={`h-7 w-7 rounded-md hover:bg-hover-bg ${isRightSidebarOpen ? "text-text-base" : "text-text-secondary"}`}
        >
          <SidebarRightIcon className="text-[15px]" />
        </Button>
      </HoverInfo>
      <HoverInfo content="打开底部终端">
        <Button
          variant="ghost"
          size="icon"
          onClick={onToggleBottomPanel}
          aria-label="打开底部终端"
          className={`h-7 w-7 rounded-md hover:bg-hover-bg ${isBottomPanelOpen ? "text-text-base" : "text-text-secondary"}`}
        >
          <BottomPanelIcon className="text-[15px]" />
        </Button>
      </HoverInfo>
    </div>
  );
}
