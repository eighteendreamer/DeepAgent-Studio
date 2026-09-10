import { useTheme, type ThemeMode } from "../../../../hooks/useTheme";
import { Sun, Moon, Monitor } from "lucide-react";
import { TEXT_PRIMARY, TEXT_SECONDARY, TEXT_MUTED, BORDER_COLOR, CARD_BG, ACCENT } from "../CanvasSettingsDialog";

interface ThemeOption {
  mode: ThemeMode;
  label: string;
  Icon: typeof Sun;
  desc: string;
}

const THEME_OPTIONS: ThemeOption[] = [
  { mode: "light", label: "浅色", Icon: Sun, desc: "明亮主题" },
  { mode: "dark", label: "深色", Icon: Moon, desc: "暗色主题" },
  { mode: "system", label: "跟随系统", Icon: Monitor, desc: "自动匹配系统设置" },
];

export function ThemeSettingsTab() {
  const { config, switchTheme } = useTheme();

  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: TEXT_PRIMARY }}>
        主题
      </h2>
      <p className="text-[12px] mb-6" style={{ color: TEXT_MUTED }}>
        与主窗口共享同一套主题状态，切换即时同步到全部窗口。画布节点区保持 Penguin 暗色风格不变。
      </p>

      <div className="space-y-3">
        {THEME_OPTIONS.map(({ mode, label, Icon, desc }) => {
          const active = config.mode === mode;
          return (
            <button
              key={mode}
              onClick={(e) => {
                const rect = e.currentTarget.getBoundingClientRect();
                switchTheme(mode, { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 });
              }}
              className="w-full flex items-center gap-3 rounded-lg border px-4 py-3 text-left transition-colors"
              style={{
                borderColor: active ? ACCENT : BORDER_COLOR,
                background: active ? "rgba(51, 156, 255, 0.1)" : CARD_BG,
              }}
            >
              <Icon
                size={18}
                strokeWidth={1.8}
                style={{ color: active ? ACCENT : TEXT_SECONDARY }}
              />
              <div className="flex-1">
                <div className="text-[13px] font-medium" style={{ color: TEXT_PRIMARY }}>
                  {label}
                </div>
                <div className="text-[11px]" style={{ color: TEXT_MUTED }}>
                  {desc}
                </div>
              </div>
              {active && (
                <div
                  className="rounded-full px-2.5 py-0.5 text-[10px] font-medium"
                  style={{ background: ACCENT, color: "#fff" }}
                >
                  当前
                </div>
              )}
            </button>
          );
        })}
      </div>
    </>
  );
}
