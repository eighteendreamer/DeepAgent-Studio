import { HoverInfo } from "./ui/HoverInfo";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import {
  checkForAvailableUpdate,
  downloadUpdateForNextShutdown,
  hasDownloadedUpdate,
  installDownloadedUpdate,
} from "../update";
import { NAV_ROW_ID, waitForDecorumElement } from "./decorumTitlebar";
import { message } from "./message";
import { Button } from "./shadcn/button";

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function currentWindow() {
  const mod = await import("@tauri-apps/api/window");
  return mod.getCurrentWindow();
}

/** Keeps update checks and shutdown installation independent from the window chrome. */
export function DesktopUpdateController() {
  const { t } = useTranslation();
  const [downloading, setDownloading] = useState(false);
  const [downloadPercent, setDownloadPercent] = useState<number | null>(null);
  const [downloadedMB, setDownloadedMB] = useState<number | null>(null);
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [navRow, setNavRow] = useState<HTMLElement | null>(null);

  useEffect(() => {
    let disposed = false;
    waitForDecorumElement(`#${NAV_ROW_ID}`)
      .then((el) => {
        if (!disposed) setNavRow(el);
      })
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, []);

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
      .then(async (appWindow) => {
        if (disposed) return;
        unlisten = await appWindow.onCloseRequested(async (event) => {
          if (installing || !hasDownloadedUpdate()) return;
          event.preventDefault();
          installing = true;
          const installed = await installDownloadedUpdate();
          if (!installed) {
            installing = false;
            message.error(t("titleBar.updateInstallFailed"));
            return;
          }
          await appWindow.close();
        });
      })
      .catch(() => {});

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [t]);

  const downloadUpdate = async () => {
    if (downloading || !updateAvailable) return;
    setDownloading(true);
    setDownloadPercent(0);
    setDownloadedMB(0);

    const ready = await downloadUpdateForNextShutdown((progress) => {
      if (typeof progress.percent === "number") {
        setDownloadPercent(progress.percent);
      }
      setDownloadedMB(Math.round((progress.downloadedBytes / 1024 / 1024) * 10) / 10);
    });

    setUpdateAvailable(!ready);
    if (ready) {
      setDownloadPercent(100);
      message.success(t("titleBar.updateReady"));
    } else {
      setDownloadPercent(null);
      setDownloadedMB(null);
      message.error(t("titleBar.updateDownloadFailed"));
    }
    setDownloading(false);
  };

  if (!updateAvailable) return null;
  if (!navRow) return null;

  return createPortal(
    <HoverInfo content={t("titleBar.downloadUpdate")}><Button
      onClick={downloadUpdate}
      disabled={downloading}

      className="relative inline-flex h-7 min-w-[104px] items-center justify-center gap-1.5 overflow-hidden rounded-full border border-blue-200 bg-blue-50 px-3 text-[12px] font-medium text-blue-700 transition-colors hover:border-blue-300 hover:bg-blue-100 disabled:cursor-default disabled:border-blue-100 disabled:bg-blue-50 disabled:text-blue-500"
    >
      {downloading && typeof downloadPercent === "number" && (
        <span
          className="pointer-events-none absolute inset-y-0 left-0 bg-blue-200/60 transition-[width] duration-200"
          style={{ width: `${downloadPercent}%` }}
        />
      )}
      <FontAwesomeIcon
        icon={["fas", downloading ? "circle-notch" : "download"]}
        className={`relative z-10 text-[11px] ${downloading ? "animate-spin" : ""}`}
      />
      <span className="relative z-10">
        {downloading
          ? typeof downloadPercent === "number" && downloadPercent > 0
            ? t("titleBar.downloadingUpdatePercent", { percent: downloadPercent })
            : typeof downloadedMB === "number" && downloadedMB > 0
              ? t("titleBar.downloadingUpdateMB", { mb: downloadedMB })
              : t("titleBar.downloadingUpdate")
          : t("titleBar.downloadUpdate")}
      </span>
    </Button></HoverInfo>,
    navRow,
  );
}
