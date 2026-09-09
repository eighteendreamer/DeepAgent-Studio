import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faBorderAll, faMinus, faSquare, faXmark } from "@fortawesome/free-solid-svg-icons";
import { useTranslation } from "react-i18next";

export type CanvasMode = "whiteboard" | "workflow";

interface CanvasTitleBarProps {
  canvasMode: CanvasMode;
  onModeChange: (mode: CanvasMode) => void;
}

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

export function CanvasTitleBar({ canvasMode, onModeChange }: CanvasTitleBarProps) {
  const { t } = useTranslation();

  const minimize = () => {
    if (inTauri()) void currentWindow().then((window) => window.minimize()).catch(() => {});
  };

  const toggleMaximize = () => {
    if (inTauri()) void currentWindow().then((window) => window.toggleMaximize()).catch(() => {});
  };

  const close = () => {
    if (inTauri()) void currentWindow().then((window) => window.close()).catch(() => {});
  };

  return (
    <div
      data-tauri-drag-region
      className="relative flex h-10 flex-shrink-0 select-none items-center border-b border-border-theme bg-sidebar-bg px-4"
    >
      <div data-tauri-drag-region className="flex min-w-0 items-center gap-2.5">
        <div className="flex h-6 w-6 items-center justify-center rounded-md bg-primary/10 text-primary">
          <FontAwesomeIcon icon={faBorderAll} className="text-[12px]" />
        </div>
        <span data-tauri-drag-region className="truncate text-[13px] font-semibold text-text-base">
          {canvasMode === "workflow"
            ? t("canvas.workflowTitle", { defaultValue: "工作流" })
            : t("canvas.title", { defaultValue: "工作画布" })}
        </span>
        <span data-tauri-drag-region className="hidden text-[12px] text-text-secondary sm:inline">
          {canvasMode === "workflow"
            ? t("canvas.workflowSubtitle", { defaultValue: "节点编排" })
            : t("canvas.subtitle", { defaultValue: "无限工作区" })}
        </span>
      </div>

      <div data-tauri-drag-region className="absolute left-1/2 top-0 flex h-10 -translate-x-1/2 items-center">
        <div className="flex items-center rounded-md bg-bg-secondary p-0.5">
          <button
            type="button"
            className={`rounded px-2.5 py-1 text-[11px] font-medium transition-colors ${
              canvasMode === "whiteboard"
                ? "bg-bg-base text-text-base shadow-sm"
                : "text-text-secondary hover:text-text-base"
            }`}
            onClick={() => onModeChange("whiteboard")}
          >
            {t("canvas.modeWhiteboard", { defaultValue: "白板" })}
          </button>
          <button
            type="button"
            className={`rounded px-2.5 py-1 text-[11px] font-medium transition-colors ${
              canvasMode === "workflow"
                ? "bg-bg-base text-text-base shadow-sm"
                : "text-text-secondary hover:text-text-base"
            }`}
            onClick={() => onModeChange("workflow")}
          >
            {t("canvas.modeWorkflow", { defaultValue: "工作流" })}
          </button>
        </div>
      </div>

      <div className="absolute right-0 top-0 flex h-10 items-center">
        <button
          type="button"
          onClick={minimize}
          className="win-btn flex h-10 w-10 items-center justify-center text-text-secondary transition-colors"
          aria-label={t("canvas.minimize", { defaultValue: "最小化" })}
        >
          <FontAwesomeIcon icon={faMinus} className="text-[11px]" />
        </button>
        <button
          type="button"
          onClick={toggleMaximize}
          className="win-btn flex h-10 w-10 items-center justify-center text-text-secondary transition-colors"
          aria-label={t("canvas.maximize", { defaultValue: "最大化" })}
        >
          <FontAwesomeIcon icon={faSquare} className="text-[11px]" />
        </button>
        <button
          type="button"
          onClick={close}
          className="win-btn-close flex h-10 w-10 items-center justify-center text-text-secondary transition-colors"
          aria-label={t("canvas.close", { defaultValue: "关闭" })}
        >
          <FontAwesomeIcon icon={faXmark} className="text-[13px]" />
        </button>
      </div>
    </div>
  );
}
