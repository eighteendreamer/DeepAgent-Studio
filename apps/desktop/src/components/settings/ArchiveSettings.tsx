import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState } from "react";

import { useTranslation } from "react-i18next";

export function ArchiveSettings() {
  const { t } = useTranslation();
  const [archives, setArchives] = useState([
    { id: 1, title: "开发", date: "2026年5月22日，14:52", project: "小红草" }
  ]);
  
  const [showToast, setShowToast] = useState(false);

  const handleUnarchive = (id: number) => {
    setArchives(archives.filter(a => a.id !== id));
    setShowToast(true);
    // Hide toast after 3 seconds
    setTimeout(() => setShowToast(false), 3000);
  };

  const handleDeleteAll = () => {
    setArchives([]);
  };

  return (
    <>
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
                <div key={chat.id} className="border border-border-theme rounded-xl p-4 flex items-center justify-between bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
                  <div>
                    <div className="text-[14px] font-medium text-text-base mb-0.5">{chat.title}</div>
                    <div className="text-[12px] text-text-secondary">
                      {chat.date} • {chat.project}
                    </div>
                  </div>
                  <div className="flex items-center space-x-4">
                    <button 
                      className="text-gray-400 hover:text-red-500 transition-colors"
                      onClick={() => setArchives(archives.filter(a => a.id !== chat.id))}
                    >
                      <FontAwesomeIcon icon={["far", "trash-can"]} className="text-[14px]" />
                    </button>
                    <button 
                      className="px-4 py-1.5 bg-gray-100 hover:bg-gray-200 rounded-full text-[12px] font-medium text-text-base transition-colors"
                      onClick={() => handleUnarchive(chat.id)}
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

      {/* Global Toast Notification */}
      {showToast && (
        <div className="fixed top-4 left-1/2 -translate-x-1/2 z-50 animate-fade-in-down">
          <div className="bg-white border border-border-theme rounded-full shadow-lg px-4 py-2 flex items-center space-x-3 text-[13px]">
            <span className="text-text-base font-medium">{t("settings.archive.chatUnarchived")}</span>
            <button className="text-blue-500 hover:underline font-medium">{t("settings.archive.viewNow")}</button>
            <button 
              className="text-gray-400 hover:text-gray-600 transition-colors ml-2"
              onClick={() => setShowToast(false)}
            >
              <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[12px]" />
            </button>
          </div>
        </div>
      )}
    </>
  );
}
