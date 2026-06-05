import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useCallback, useEffect, useState } from "react";

import { useTranslation } from "react-i18next";
import {
  deleteAllArchivedConversations,
  deleteArchivedConversation,
  listArchivedConversations,
  unarchiveConversation,
} from "../../api";
import type { ArchivedConversation } from "../../types";
import { message } from "../message";

function formatArchiveDate(timestamp: number): string {
  if (!timestamp) return "";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "long",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}

export function ArchiveSettings() {
  const { t } = useTranslation();
  const [archives, setArchives] = useState<ArchivedConversation[]>([]);

  const reload = useCallback(() => {
    listArchivedConversations()
      .then(setArchives)
      .catch(() => {
        setArchives([]);
        message.error(t("settings.archive.loadFailed"));
      });
  }, [t]);

  useEffect(() => {
    reload();
  }, [reload]);

  const handleUnarchive = (sessionId: string) => {
    unarchiveConversation(sessionId)
      .then(() => {
        setArchives((prev) => prev.filter((a) => a.session_id !== sessionId));
        message.success(t("settings.archive.chatUnarchived"));
      })
      .catch(() => message.error(t("settings.archive.actionFailed")));
  };

  const handleDelete = (sessionId: string) => {
    deleteArchivedConversation(sessionId)
      .then(() => {
        setArchives((prev) => prev.filter((a) => a.session_id !== sessionId));
        message.success(t("settings.archive.archiveDeleted"));
      })
      .catch(() => message.error(t("settings.archive.actionFailed")));
  };

  const handleDeleteAll = () => {
    deleteAllArchivedConversations()
      .then(() => {
        setArchives([]);
        message.success(t("settings.archive.archiveCleared"));
      })
      .catch(() => message.error(t("settings.archive.actionFailed")));
  };

  return (
      <div className="pb-20 relative">
        <div className="mb-10 max-w-[700px] flex items-end justify-between">
          <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.archive.title")}</h1>
          {archives.length > 0 && (
            <button 
              className="px-3 py-1.5 bg-red-100 hover:bg-red-200 text-red-600 rounded-full text-[12px] font-medium transition-colors"
              onClick={handleDeleteAll}
            >
              {t("settings.archive.deleteAll")}
            </button>
          )}
        </div>

        <div className="max-w-[700px]">
          {archives.length > 0 ? (
            <div className="space-y-3">
              {archives.map((chat) => (
                <div key={chat.session_id} className="border border-border-theme rounded-xl p-4 flex items-center justify-between bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
                  <div>
                    <div className="text-[14px] font-medium text-text-base mb-0.5">
                      {chat.title || t("settings.archive.untitled")}
                    </div>
                    <div className="text-[12px] text-text-secondary">
                      {formatArchiveDate(chat.archived_at)} • {chat.project || t("settings.archive.unknownProject")}
                    </div>
                  </div>
                  <div className="flex items-center space-x-4">
                    <button 
                      className="text-gray-400 hover:text-red-500 transition-colors"
                      onClick={() => handleDelete(chat.session_id)}
                    >
                      <FontAwesomeIcon icon={["far", "trash-can"]} className="text-[14px]" />
                    </button>
                    <button 
                      className="px-4 py-1.5 bg-gray-100 hover:bg-gray-200 rounded-full text-[12px] font-medium text-text-base transition-colors"
                      onClick={() => handleUnarchive(chat.session_id)}
                    >
                      {t("settings.archive.unarchive")}
                    </button>
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div className="border border-border-theme rounded-xl p-4 bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] flex justify-start items-center h-[60px]">
              <span className="text-[13px] text-text-secondary">{t("settings.archive.noArchivedChats")}</span>
            </div>
          )}
        </div>
      </div>
  );
}
