import { useEffect, useState } from "react";
import {
  ProjectMapDebugView,
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
    <div>
      <div className="mb-8 flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold text-text-base mb-1">项目地图调试</h1>
          <p className="text-[13px] text-text-secondary">
            用于排查项目地图是否可用、是否过期，以及当前加载的 JSON 路径。
          </p>
        </div>
        <SettingsSwitch enabled={buttonVisible} onChange={updateButtonVisible} />
      </div>
      <ProjectMapDebugView />
    </div>
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
      className={`min-w-[176px] rounded-lg border px-3 py-2 text-left transition-colors ${
        enabled
          ? "border-gray-900 bg-gray-900 text-white"
          : "border-border-theme bg-white text-text-base hover:bg-gray-50"
      }`}
      onClick={() => onChange(!enabled)}
    >
      <div className="flex items-center justify-between gap-3">
        <span className="text-[13px] font-medium">显示面板 Debug 按钮</span>
        <span
          className={`relative h-5 w-9 rounded-full transition-colors ${
            enabled ? "bg-white/30" : "bg-gray-200"
          }`}
        >
          <span
            className={`absolute top-0.5 h-4 w-4 rounded-full bg-white shadow-sm transition-transform ${
              enabled ? "translate-x-4" : "translate-x-0.5"
            }`}
          />
        </span>
      </div>
      <div className={`mt-1 text-[11px] ${enabled ? "text-white/75" : "text-text-secondary"}`}>
        {enabled ? "项目地图面板会显示 Debug 入口" : "项目地图面板隐藏 Debug 入口"}
      </div>
    </button>
  );
}
