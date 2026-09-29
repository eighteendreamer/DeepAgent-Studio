import { useEffect, useState } from "react";
import {
  readProjectMapDebugButtonVisible,
  writeProjectMapDebugButtonVisible,
  writeProjectMapDebugEnabled,
} from "../project-map/ProjectMapDebugView";

export function ProjectMapDebugSettings() {
  const [buttonVisible, setButtonVisible] = useState(() => readProjectMapDebugButtonVisible());

  const updateButtonVisible = (next: boolean) => {
    setButtonVisible(next);
    writeProjectMapDebugButtonVisible(next);
    if (!next) {
      writeProjectMapDebugEnabled(false);
    }
  };

  useEffect(() => {
    const onDebugButtonVisibleChanged = (event: Event) => {
      setButtonVisible(Boolean((event as CustomEvent<boolean>).detail));
    };
    window.addEventListener("deepagent:project-map-debug-button-visible-changed", onDebugButtonVisibleChanged);
    return () => window.removeEventListener("deepagent:project-map-debug-button-visible-changed", onDebugButtonVisibleChanged);
  }, []);

  return (
    <section className="w-full space-y-8">
      <header className="space-y-1">
        <h1 className="text-2xl font-semibold text-text-base">项目地图调试</h1>
        <p className="text-[13px] text-text-secondary">
          管理项目地图面板中的 Debug 按钮显示。
        </p>
      </header>

      <div className="rounded-xl border border-border-theme bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
        <div className="flex items-center justify-between gap-6 p-4">
          <div className="min-w-0">
            <h2 className="text-[14px] font-medium text-text-base">显示面板 Debug 按钮</h2>
            <p className="mt-1 text-[12px] text-text-secondary">
              {buttonVisible
                ? "项目地图面板顶部会显示 Debug 入口。"
                : "项目地图面板顶部不会显示 Debug 入口。"}
            </p>
          </div>
          <SettingsSwitch enabled={buttonVisible} onChange={updateButtonVisible} />
        </div>
      </div>
    </section>
  );
}

function SettingsSwitch({
  enabled,
  onChange,
}: {
  enabled: boolean;
  onChange: (enabled: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={enabled}
      aria-label="显示面板 Debug 按钮"
      className={`relative h-6 w-11 shrink-0 rounded-full transition-colors ${
        enabled
          ? "bg-gray-900"
          : "bg-gray-200 hover:bg-gray-300"
      }`}
      onClick={() => onChange(!enabled)}
    >
      <span
        className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow-sm transition-transform ${
          enabled ? "translate-x-5" : "translate-x-0.5"
        }`}
      />
    </button>
  );
}
