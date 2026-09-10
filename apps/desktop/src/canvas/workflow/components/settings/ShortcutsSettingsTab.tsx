interface CanvasShortcut {
  name: string;
  desc: string;
  keys: string[];
}

const CANVAS_SHORTCUTS: CanvasShortcut[] = [
  { name: "取消选择", desc: "清除所有选中节点", keys: ["Escape"] },
  { name: "全选", desc: "选中画布上所有节点", keys: ["Ctrl+A"] },
  { name: "适应屏幕", desc: "自动缩放以显示全部节点", keys: ["F"] },
  { name: "复制节点", desc: "复制选中节点", keys: ["Ctrl+C"] },
  { name: "粘贴节点", desc: "粘贴节点（偏移 40px）", keys: ["Ctrl+V"] },
  { name: "平移画布", desc: "按住空格拖拽平移", keys: ["Space+拖拽"] },
  { name: "撤销", desc: "撤销上一步操作", keys: ["Ctrl+Z"] },
  { name: "重做", desc: "恢复已撤销的操作", keys: ["Ctrl+Shift+Z", "Ctrl+Y"] },
  { name: "删除节点", desc: "删除选中节点", keys: ["Delete", "Backspace"] },
];

function ShortcutRow({ name, desc, keys }: CanvasShortcut) {
  return (
    <div
      className="flex items-center justify-between px-4 py-3 border-b last:border-b-0"
      style={{ borderColor: "var(--theme-border, rgba(0,0,0,0.08))" }}
    >
      <div>
        <div className="text-[13px] font-medium" style={{ color: "var(--theme-fg, #111)" }}>
          {name}
        </div>
        <div className="text-[11px]" style={{ color: "var(--theme-text-secondary, #999)" }}>
          {desc}
        </div>
      </div>
      <div className="flex items-center gap-1.5">
        {keys.map((k, i) => (
          <span
            key={i}
            className="rounded px-2 py-0.5 text-[11px] font-mono"
            style={{
              background: "rgba(0,0,0,0.05)",
              color: "var(--theme-text-secondary, #666)",
              border: "1px solid var(--theme-border, rgba(0,0,0,0.08))",
            }}
          >
            {k}
          </span>
        ))}
      </div>
    </div>
  );
}

export function ShortcutsSettingsTab() {
  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: "var(--theme-fg, #111)" }}>
        快捷键设置
      </h2>
      <p className="text-[12px] mb-6" style={{ color: "var(--theme-text-secondary, #666)" }}>
        画布专属快捷键列表，系统快捷键请在主窗口设置中查看。
      </p>

      <div className="rounded-lg border overflow-hidden" style={{ borderColor: "var(--theme-border, rgba(0,0,0,0.08))" }}>
        {CANVAS_SHORTCUTS.map((s) => (
          <ShortcutRow key={s.name} {...s} />
        ))}
      </div>
    </>
  );
}
