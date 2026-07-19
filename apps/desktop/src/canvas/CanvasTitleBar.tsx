import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

export function CanvasTitleBar() {
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
          <FontAwesomeIcon icon={["fas", "border-all"]} className="text-[12px]" />
        </div>
        <span data-tauri-drag-region className="truncate text-[13px] font-semibold text-text-base">
          {t("canvas.title", { defaultValue: "工作画布" })}
        </span>
        <span data-tauri-drag-region className="hidden text-[12px] text-text-secondary sm:inline">
          {t("canvas.subtitle", { defaultValue: "无限工作区" })}
        </span>
      </div>

      <div className="absolute right-0 top-0 flex h-10 items-center">
        <button
          type="button"
          onClick={minimize}
          className="win-btn flex h-10 w-10 items-center justify-center text-text-secondary transition-colors"
          aria-label={t("canvas.minimize", { defaultValue: "最小化" })}
        >
          <FontAwesomeIcon icon={["fas", "minus"]} className="text-[11px]" />
        </button>
        <button
          type="button"
          onClick={toggleMaximize}
          className="win-btn flex h-10 w-10 items-center justify-center text-text-secondary transition-colors"
          aria-label={t("canvas.maximize", { defaultValue: "最大化" })}
        >
          <FontAwesomeIcon icon={["far", "square"]} className="text-[11px]" />
        </button>
        <button
          type="button"
          onClick={close}
          className="win-btn-close flex h-10 w-10 items-center justify-center text-text-secondary transition-colors"
          aria-label={t("canvas.close", { defaultValue: "关闭" })}
        >
          <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[13px]" />
        </button>
      </div>
    </div>
  );
}
