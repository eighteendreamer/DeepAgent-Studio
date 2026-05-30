import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { SidebarLeftIcon } from "./icons";
import { useTranslation } from "react-i18next";

interface Props {
  onToggleSidebar: () => void;
  isSidebarOpen: boolean;
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
}

/** Whether we're running inside the Tauri desktop shell. */
function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Lazily get the current window handle (only inside Tauri). */
async function currentWindow() {
  const mod = await import("@tauri-apps/api/window");
  return mod.getCurrentWindow();
}

export function TitleBar({ onToggleSidebar, isSidebarOpen, canGoBack, canGoForward, onBack, onForward }: Props) {
  const { t } = useTranslation();
  const MENUS = [
    { key: "file", label: t("titleBar.file") },
    { key: "edit", label: t("titleBar.edit") },
    { key: "view", label: t("titleBar.view") },
    { key: "window", label: t("titleBar.window") },
    { key: "help", label: t("titleBar.help") }
  ];

  const onMinimize = () => {
    if (inTauri()) currentWindow().then((w) => w.minimize()).catch(() => {});
  };
  const onToggleMaximize = () => {
    if (inTauri()) currentWindow().then((w) => w.toggleMaximize()).catch(() => {});
  };
  const onClose = () => {
    if (inTauri()) currentWindow().then((w) => w.close()).catch(() => {});
  };

  return (
    <div
      data-tauri-drag-region
      className="h-10 w-full flex items-center justify-between px-4 flex-shrink-0 no-select"
    >
      {/* Left: nav + menus */}
      <div className="flex items-center space-x-4 text-text-secondary text-sm">
        <SidebarLeftIcon
          className={`cursor-pointer transition-colors ${!isSidebarOpen ? "text-text-base" : "hover:text-text-base"}`}
          onClick={onToggleSidebar}
        />
        <FontAwesomeIcon 
          icon={["fas", "arrow-left"]} 
          className={`cursor-pointer transition-colors ml-2 ${canGoBack ? "text-text-secondary hover:text-text-base" : "text-gray-300 cursor-not-allowed"}`} 
          onClick={() => canGoBack && onBack()}
        />
        <FontAwesomeIcon
          icon={["fas", "arrow-right"]}
          className={`cursor-pointer transition-colors ${canGoForward ? "text-text-secondary hover:text-text-base" : "text-gray-300 cursor-not-allowed"}`}
          onClick={() => canGoForward && onForward()}
        />
        <div className="flex space-x-4 ml-4">
          {MENUS.map((m) => (
            <span key={m.key} className="cursor-pointer hover:text-text-base transition-colors">
              {m.label}
            </span>
          ))}
        </div>
      </div>

      {/* Right: window controls */}
      <div className="flex items-center -mr-4">
        <button
          className="win-btn w-10 h-10 flex items-center justify-center text-text-secondary transition-colors"
          onClick={onMinimize}
        >
          <FontAwesomeIcon icon={["fas", "minus"]} className="text-xs" />
        </button>
        <button
          className="win-btn w-10 h-10 flex items-center justify-center text-text-secondary transition-colors"
          onClick={onToggleMaximize}
        >
          <FontAwesomeIcon icon={["far", "square"]} className="text-xs" />
        </button>
        <button
          className="win-btn-close w-10 h-10 flex items-center justify-center text-text-secondary transition-colors"
          onClick={onClose}
        >
          <FontAwesomeIcon icon={["fas", "xmark"]} className="text-sm" />
        </button>
      </div>
    </div>
  );
}
