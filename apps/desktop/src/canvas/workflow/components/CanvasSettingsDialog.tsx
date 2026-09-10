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

const NAV_BG = "rgba(255,255,255,0.03)";
const NAV_BORDER = "rgba(255,255,255,0.08)";
const NAV_ITEM_ACTIVE = "rgba(255,255,255,0.08)";
const NAV_ITEM_HOVER = "rgba(255,255,255,0.05)";
const TEXT_PRIMARY = "rgba(255,255,255,0.92)";
const TEXT_SECONDARY = "rgba(255,255,255,0.5)";
const TEXT_MUTED = "rgba(255,255,255,0.35)";
const BORDER_COLOR = "rgba(255,255,255,0.08)";
const CARD_BG = "rgba(255,255,255,0.04)";
const INPUT_BG = "rgba(255,255,255,0.06)";
const ACCENT = "#339CFF";

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
        className="p-0"
        style={{
          width: 860,
          height: 560,
          maxWidth: "90vw",
          maxHeight: "85vh",
          background: "#0d0d0d",
          color: TEXT_PRIMARY,
          border: `1px solid ${NAV_BORDER}`,
        }}
      >
        <DialogTitle className="sr-only">画布设置</DialogTitle>
        <div className="flex h-full">
          <nav
            style={{
              width: 180,
              borderRight: `1px solid ${NAV_BORDER}`,
              background: NAV_BG,
              padding: "12px 8px",
              flexShrink: 0,
            }}
          >
            {TABS.map(({ id, label, Icon }) => {
              const active = activeTab === id;
              return (
                <button
                  key={id}
                  onClick={() => setActiveTab(id)}
                  className="flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-[13px] font-medium transition-colors"
                  style={{
                    background: active ? NAV_ITEM_ACTIVE : "transparent",
                    color: active ? TEXT_PRIMARY : TEXT_SECONDARY,
                  }}
                  onMouseEnter={(e) => {
                    if (!active) e.currentTarget.style.background = NAV_ITEM_HOVER;
                  }}
                  onMouseLeave={(e) => {
                    if (!active) e.currentTarget.style.background = "transparent";
                  }}
                >
                  <Icon size={15} strokeWidth={1.8} />
                  {label}
                </button>
              );
            })}
          </nav>
          <div className="min-w-0 flex-1 overflow-y-auto overflow-x-hidden px-8 py-6">
            <TabContent tab={activeTab} />
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export { TEXT_PRIMARY, TEXT_SECONDARY, TEXT_MUTED, BORDER_COLOR, CARD_BG, INPUT_BG, ACCENT };
