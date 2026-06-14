import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { SidebarLeftIcon } from "./icons";
import { useTranslation } from "react-i18next";
import { useEffect, useState } from "react";
import {
  checkForAvailableUpdate,
  downloadUpdateForNextShutdown,
  hasDownloadedUpdate,
  installDownloadedUpdate,
} from "../update";
import { message } from "./message";

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
  const [downloadingUpdate, setDownloadingUpdate] = useState(false);
  const [updateAvailable, setUpdateAvailable] = useState(false);
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
    if (!inTauri()) return;
    currentWindow()
      .then((w) => w.close())
      .catch(() => {});
  };

  const onDownloadUpdate = async () => {
    if (downloadingUpdate || !updateAvailable) return;
    setDownloadingUpdate(true);
    const ready = await downloadUpdateForNextShutdown();
    setUpdateAvailable(!ready);
    if (ready) {
      message.success(t("titleBar.updateReady"));
    } else {
      message.error(t("titleBar.updateDownloadFailed"));
    }
    setDownloadingUpdate(false);
  };

  useEffect(() => {
    if (!inTauri()) return;
    let disposed = false;
    let installing = false;
    let unlisten: (() => void) | undefined;

    checkForAvailableUpdate()
      .then((available) => {
        if (!disposed) setUpdateAvailable(available);
      })
      .catch(() => {
        if (!disposed) setUpdateAvailable(false);
      });

    currentWindow()
      .then(async (w) => {
        if (disposed) return;
        unlisten = await w.onCloseRequested(async (event) => {
          if (installing || !hasDownloadedUpdate()) return;
          event.preventDefault();
          installing = true;
          const installed = await installDownloadedUpdate();
          if (!installed) {
            installing = false;
            message.error(t("titleBar.updateInstallFailed"));
            return;
          }
          await w.close();
        });
      })
      .catch(() => {});

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

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
          {updateAvailable && (
            <button
              type="button"
              onClick={onDownloadUpdate}
              disabled={downloadingUpdate}
              title={t("titleBar.downloadUpdate")}
              className="ml-1 inline-flex h-7 items-center gap-1.5 rounded-full border border-blue-200 bg-blue-50 px-2.5 text-[12px] font-medium text-blue-700 shadow-sm transition-colors hover:border-blue-300 hover:bg-blue-100 disabled:cursor-default disabled:border-blue-100 disabled:bg-blue-50/60 disabled:text-blue-500"
            >
              <FontAwesomeIcon
                icon={["fas", downloadingUpdate ? "circle-notch" : "download"]}
                className={downloadingUpdate ? "animate-spin text-[11px]" : "text-[11px]"}
              />
              <span>{t("titleBar.downloadUpdate")}</span>
            </button>
          )}
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
