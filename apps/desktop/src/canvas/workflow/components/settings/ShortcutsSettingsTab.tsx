import { useState } from "react";
import { getShortcutsData, type Shortcut } from "../../../../components/settings/shortcutsData";
import { useTranslation } from "react-i18next";

interface CanvasShortcut {
  name: string;
  keys: string[];
}

const CANVAS_SHORTCUTS: CanvasShortcut[] = [
  { name: "取消选择", keys: ["Escape"] },
  { name: "全选", keys: ["Ctrl+A"] },
  { name: "适应屏幕", keys: ["F"] },
  { name: "复制节点", keys: ["Ctrl+C"] },
  { name: "粘贴节点", keys: ["Ctrl+V"] },
  { name: "平移画布", keys: ["Space+拖拽"] },
  { name: "撤销", keys: ["Ctrl+Z"] },
  { name: "重做", keys: ["Ctrl+Shift+Z"] },
  { name: "删除节点", keys: ["Delete"] },
];

function ShortcutRow({ name, desc, keys }: { name: string; desc?: string; keys: string[] }) {
  return (
    <div
      className="flex items-center justify-between px-4 py-2.5 border-b last:border-b-0"
      style={{ borderColor: "var(--theme-border, #ddd)" }}
    >
      <div>
        <div className="text-[13px] font-medium" style={{ color: "var(--theme-fg, #111)" }}>
          {name}
        </div>
        {desc && (
          <div className="text-[11px]" style={{ color: "var(--theme-text-secondary, #999)" }}>
            {desc}
          </div>
        )}
      </div>
      <div className="flex items-center gap-1.5">
        {keys.map((k, i) => (
          <span
            key={i}
            className="rounded px-2 py-0.5 text-[11px] font-mono"
            style={{
              background: "rgba(0,0,0,0.05)",
              color: "var(--theme-text-secondary, #666)",
              border: "1px solid var(--theme-border, #ddd)",
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
  const { t } = useTranslation();
  const [search, setSearch] = useState("");

  const allShortcuts: Shortcut[] = getShortcutsData(t as any);
  const filtered = allShortcuts.filter(
    (s) =>
      s.name.toLowerCase().includes(search.toLowerCase()) ||
      s.desc.toLowerCase().includes(search.toLowerCase()),
  );

  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: "var(--theme-fg, #111)" }}>
        快捷键设置
      </h2>
      <p className="text-[12px] mb-6" style={{ color: "var(--theme-text-secondary, #666)" }}>
        画布快捷键与系统主体保持一致，以下为完整快捷键列表。
      </p>

      {/* Canvas-specific shortcuts */}
      <div className="mb-6">
        <h3 className="text-[13px] font-semibold mb-2" style={{ color: "var(--theme-fg, #111)" }}>
          画布专属
        </h3>
        <div
          className="rounded-lg border overflow-hidden"
          style={{ borderColor: "var(--theme-border, #ddd)" }}
        >
          {CANVAS_SHORTCUTS.map((s) => (
            <ShortcutRow key={s.name} name={s.name} keys={s.keys} />
          ))}
        </div>
      </div>

      {/* System shortcuts */}
      <div>
        <div className="flex items-center justify-between mb-2">
          <h3 className="text-[13px] font-semibold" style={{ color: "var(--theme-fg, #111)" }}>
            系统快捷键
          </h3>
          <input
            type="text"
            placeholder="搜索..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="h-7 rounded-md border px-2.5 text-[12px] focus:outline-none focus:border-blue-500"
            style={{
              borderColor: "var(--theme-border, #ddd)",
              background: "var(--theme-bg, #fff)",
              color: "var(--theme-fg, #111)",
              width: 160,
            }}
          />
        </div>
        <div
          className="rounded-lg border overflow-hidden max-h-[280px] overflow-y-auto"
          style={{ borderColor: "var(--theme-border, #ddd)" }}
        >
          {filtered.length === 0 ? (
            <div className="px-4 py-6 text-center text-[12px]" style={{ color: "var(--theme-text-secondary, #999)" }}>
              无匹配结果
            </div>
          ) : (
            filtered.map((s, i) => (
              <ShortcutRow key={i} name={s.name} desc={s.desc} keys={s.keys} />
            ))
          )}
        </div>
      </div>
    </>
  );
}
