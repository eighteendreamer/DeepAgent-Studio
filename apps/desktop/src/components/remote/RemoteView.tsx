import { File as FileIcon, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { SshConnection, SshDirEntry } from "../../api";

interface RemoteViewProps {
  connection: SshConnection | null;
  file: SshDirEntry | null;
}

const statusClassName = (status: SshConnection["status"]) => {
  switch (status) {
    case "connected":
      return "bg-green-100 text-green-700";
    case "connecting":
      return "bg-yellow-100 text-yellow-700";
    case "error":
      return "bg-red-100 text-red-700";
    default:
      return "bg-gray-100 text-text-secondary";
  }
};

const statusLabel = (status: SshConnection["status"], t: (key: string) => string) => {
  switch (status) {
    case "connected":
      return t("settings.connections.online");
    case "connecting":
      return t("settings.connections.checking");
    default:
      return t("settings.connections.offline");
  }
};

const formatSize = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
};

export function RemoteView({ connection, file }: RemoteViewProps) {
  const { t } = useTranslation();

  if (!connection) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center px-8">
        <Server className="mb-4 h-10 w-10 text-gray-300" />
        <div className="mb-1.5 text-[15px] font-medium text-text-base">
          {t("remote.emptyTitle")}
        </div>
        <div className="text-[13px] text-text-secondary">
          {t("remote.emptyDesc")}
        </div>
      </div>
    );
  }

  return (
    <div className="h-full w-full overflow-y-auto px-8 py-6">
      <h1 className="mb-6 text-2xl font-semibold text-text-base">
        {t("remote.title")}
      </h1>

      <div className="mb-4 max-w-[720px] rounded-xl border border-border-theme bg-white p-4 shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
        <div className="flex items-center gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-black/5 text-gray-600">
            <Server className="h-4 w-4" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium text-text-base">
              {connection.name}
            </div>
            <div className="break-all text-[12px] text-text-secondary">
              {connection.username}@{connection.host}:{connection.port}
            </div>
          </div>
          <span
            className={`rounded-full px-2 py-0.5 text-[11px] ${statusClassName(connection.status)}`}
          >
            {statusLabel(connection.status, t)}
          </span>
          {typeof connection.latency_ms === "number" && (
            <span className="text-[11px] text-text-secondary">
              {connection.latency_ms}ms
            </span>
          )}
        </div>
      </div>

      {file && (
        <div className="max-w-[720px] rounded-xl border border-border-theme bg-white p-4 shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-center gap-3">
            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-black/5 text-gray-600">
              <FileIcon className="h-4 w-4" />
            </div>
            <div className="min-w-0">
              <div className="truncate text-[13px] font-medium text-text-base">
                {file.name}
              </div>
              <div className="break-all text-[11px] text-text-secondary">
                {file.path}
              </div>
            </div>
          </div>
          <div className="mt-3 grid grid-cols-2 gap-2 text-[12px]">
            <div>
              <span className="text-text-secondary">{t("remote.size")}: </span>
              <span className="text-text-base">
                {typeof file.size === "number" ? formatSize(file.size) : "—"}
              </span>
            </div>
            <div>
              <span className="text-text-secondary">
                {t("remote.modifiedAt")}:{" "}
              </span>
              <span className="text-text-base">
                {file.modified_ms
                  ? new Date(file.modified_ms).toLocaleString()
                  : "—"}
              </span>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
