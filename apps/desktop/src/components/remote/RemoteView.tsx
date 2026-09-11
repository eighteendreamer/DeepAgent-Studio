import { Loader2, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import { TerminalPlugin } from "../plugins/TerminalPlugin";
import type { SshConnection } from "../../api";

interface RemoteViewProps {
  connection: SshConnection | null;
}

export function RemoteView({ connection }: RemoteViewProps) {
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

  if (connection.status !== "connected") {
    const failed = connection.status === "error";
    return (
      <div className="flex h-full w-full flex-col items-center justify-center px-8">
        {failed ? (
          <Server className="mb-4 h-10 w-10 text-gray-300" />
        ) : (
          <Loader2 className="mb-4 h-8 w-8 animate-spin text-gray-400" />
        )}
        <div className="mb-1.5 text-[15px] font-medium text-text-base">
          {failed ? t("remote.connectFailed") : t("remote.connecting")}
        </div>
        <div className="text-[13px] text-text-secondary">
          {connection.username}@{connection.host}:{connection.port}
        </div>
        {failed && (
          <div className="text-[12px] text-gray-400">
            {t("remote.reconnectHint")}
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="h-full w-full">
      <TerminalPlugin mode="remote" connectionId={connection.id} />
    </div>
  );
}
