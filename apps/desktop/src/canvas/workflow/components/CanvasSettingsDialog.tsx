import { useState } from "react";
import {
  Cpu,
  FolderOpen,
  Keyboard,
  Palette,
  type LucideIcon,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "../../../components/shadcn/dialog";
import { useCanvasStore } from "../store/canvasStore";
import { ThemeSettingsTab } from "./settings/ThemeSettingsTab";
import { ModelSettingsTab } from "./settings/ModelSettingsTab";
import { WorkspaceSettingsTab } from "./settings/WorkspaceSettingsTab";
import { ShortcutsSettingsTab } from "./settings/ShortcutsSettingsTab";

type TabId = "theme" | "model" | "workspace" | "shortcuts";

interface TabDef {
  id: TabId;
  label: string;
  Icon: LucideIcon;
}

const TABS: TabDef[] = [
  { id: "theme", label: "主题", Icon: Palette },
  { id: "model", label: "模型", Icon: Cpu },
  { id: "workspace", label: "工作区", Icon: FolderOpen },
  { id: "shortcuts", label: "快捷键", Icon: Keyboard },
];

const NAV_STYLE: React.CSSProperties = {
  width: 180,
  borderRight: "1px solid var(--theme-border, rgba(0,0,0,0.08))",
  padding: "12px 8px",
  flexShrink: 0,
};

const NAV_ITEM_CLASS =
  "flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-[13px] font-medium transition-colors";

function TabContent({ tab }: { tab: TabId }) {
  switch (tab) {
    case "theme":
      return <ThemeSettingsTab />;
    case "model":
      return <ModelSettingsTab />;
    case "workspace":
      return <WorkspaceSettingsTab />;
    case "shortcuts":
      return <ShortcutsSettingsTab />;
  }
}

export function CanvasSettingsDialog() {
  const settingsOpen = useCanvasStore((s) => s.settingsOpen);
  const closeSettings = useCanvasStore((s) => s.closeSettings);
  const [activeTab, setActiveTab] = useState<TabId>("theme");

  return (
    <Dialog open={settingsOpen} onOpenChange={(open) => { if (!open) closeSettings(); }}>
      <DialogContent
        className="overflow-hidden p-0"
        style={{
          width: 760,
          height: 560,
          maxWidth: "90vw",
          maxHeight: "85vh",
          background: "var(--theme-bg, #fff)",
          color: "var(--theme-fg, #111)",
          border: "1px solid var(--theme-border, rgba(0,0,0,0.08))",
        }}
      >
        <DialogTitle className="sr-only">画布设置</DialogTitle>
        <div className="flex h-full">
          <nav style={NAV_STYLE}>
            {TABS.map(({ id, label, Icon }) => {
              const active = activeTab === id;
              return (
                <button
                  key={id}
                  onClick={() => setActiveTab(id)}
                  className={NAV_ITEM_CLASS}
                  style={{
                    background: active ? "rgba(0,0,0,0.06)" : "transparent",
                    color: active ? "var(--theme-fg, #111)" : "var(--theme-text-secondary, #666)",
                  }}
                >
                  <Icon size={15} strokeWidth={1.8} />
                  {label}
                </button>
              );
            })}
          </nav>
          <div className="min-w-0 flex-1 overflow-y-auto px-8 py-6">
            <TabContent tab={activeTab} />
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
