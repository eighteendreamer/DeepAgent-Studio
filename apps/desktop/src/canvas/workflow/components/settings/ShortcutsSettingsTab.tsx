import {
  MousePointer2,
  Square,
  Maximize2,
  Copy,
  Clipboard,
  Hand,
  Undo2,
  Redo2,
  Trash2,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { TEXT_PRIMARY, TEXT_SECONDARY, TEXT_MUTED, BORDER_COLOR, CARD_BG } from "../CanvasSettingsDialog";

interface CanvasShortcut {
  name: string;
  desc: string;
  keys: string[];
  Icon: LucideIcon;
  group: "navigation" | "edit";
}

const CANVAS_SHORTCUTS: CanvasShortcut[] = [
  { name: "取消选择", desc: "清除所有选中节点", keys: ["Esc"], Icon: MousePointer2, group: "navigation" },
  { name: "全选", desc: "选中画布上所有节点", keys: ["Ctrl", "A"], Icon: Square, group: "navigation" },
  { name: "适应屏幕", desc: "自动缩放以显示全部节点", keys: ["F"], Icon: Maximize2, group: "navigation" },
  { name: "平移画布", desc: "按住空格拖拽平移", keys: ["Space", "+ 拖拽"], Icon: Hand, group: "navigation" },
  { name: "复制节点", desc: "复制选中节点", keys: ["Ctrl", "C"], Icon: Copy, group: "edit" },
  { name: "粘贴节点", desc: "粘贴节点（偏移 40px）", keys: ["Ctrl", "V"], Icon: Clipboard, group: "edit" },
  { name: "撤销", desc: "撤销上一步操作", keys: ["Ctrl", "Z"], Icon: Undo2, group: "edit" },
  {
    name: "重做",
    desc: "恢复已撤销的操作（也可使用 Ctrl + Y）",
    keys: ["Ctrl", "Shift", "Z"],
    Icon: Redo2,
    group: "edit",
  },
  {
    name: "删除节点",
    desc: "删除选中节点",
    keys: ["Delete / Backspace"],
    Icon: Trash2,
    group: "edit",
  },
];

function KeyCap({ children }: { children: string }) {
  return (
    <span
      className="inline-flex items-center justify-center rounded px-1.5 py-0.5 text-[10px] font-mono font-medium"
      style={{
        background: "rgba(255,255,255,0.08)",
        color: TEXT_PRIMARY,
        border: "1px solid rgba(255,255,255,0.12)",
        minWidth: "20px",
      }}
    >
      {children}
    </span>
  );
}

function ShortcutCard({ shortcut }: { shortcut: CanvasShortcut }) {
  const { name, desc, keys, Icon } = shortcut;
  return (
    <div
      className="rounded-lg p-3 transition-colors"
      style={{
        background: CARD_BG,
        border: `1px solid ${BORDER_COLOR}`,
      }}
    >
      <div className="flex items-start justify-between gap-2 mb-2">
        <div className="flex items-center gap-2">
          <Icon size={14} strokeWidth={1.8} style={{ color: TEXT_SECONDARY }} />
          <span className="text-[12px] font-medium" style={{ color: TEXT_PRIMARY }}>
            {name}
          </span>
        </div>
        <div className="flex items-center gap-1">
          {keys.map((k, i) => (
            <span key={i} className="flex items-center gap-1">
              {i > 0 && <span style={{ color: TEXT_MUTED }}>+</span>}
              <KeyCap>{k}</KeyCap>
            </span>
          ))}
        </div>
      </div>
      <div className="text-[10px]" style={{ color: TEXT_MUTED }}>
        {desc}
      </div>
    </div>
  );
}

export function ShortcutsSettingsTab() {
  const navShortcuts = CANVAS_SHORTCUTS.filter((s) => s.group === "navigation");
  const editShortcuts = CANVAS_SHORTCUTS.filter((s) => s.group === "edit");

  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: TEXT_PRIMARY }}>
        快捷键
      </h2>
      <p className="text-[12px] mb-5" style={{ color: TEXT_MUTED }}>
        画布专属快捷键，系统快捷键请在主窗口设置中查看。
      </p>

      <div className="space-y-4">
        <div>
          <div className="text-[11px] font-medium mb-2 uppercase tracking-wider" style={{ color: TEXT_MUTED }}>
            画布导航
          </div>
          <div className="grid grid-cols-2 gap-2">
            {navShortcuts.map((s) => (
              <ShortcutCard key={s.name} shortcut={s} />
            ))}
          </div>
        </div>

        <div>
          <div className="text-[11px] font-medium mb-2 uppercase tracking-wider" style={{ color: TEXT_MUTED }}>
            节点编辑
          </div>
          <div className="grid grid-cols-2 gap-2">
            {editShortcuts.map((s) => (
              <ShortcutCard key={s.name} shortcut={s} />
            ))}
          </div>
        </div>
      </div>
    </>
  );
}
