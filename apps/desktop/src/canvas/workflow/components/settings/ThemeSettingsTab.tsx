import { AppearanceThemeSection } from "../../../../components/settings/AppearanceThemeSection";

export function ThemeSettingsTab() {
  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: "var(--theme-fg, #111)" }}>
        主题设置
      </h2>
      <p className="text-[12px] mb-6" style={{ color: "var(--theme-text-secondary, #666)" }}>
        与主窗口共享同一套主题状态，切换即时同步到全部窗口。画布节点区保持 Penguin 暗色风格不变。
      </p>
      <AppearanceThemeSection />
    </>
  );
}
