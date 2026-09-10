import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { useState } from "react";
import { getShortcutsData } from "./shortcutsData";

export function ShortcutsSettings() {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");

  const filteredShortcuts = getShortcutsData(t).filter(
    (s) =>
      s.name.toLowerCase().includes(search.toLowerCase()) ||
      s.desc.toLowerCase().includes(search.toLowerCase())
  );

  return (
    <>
      <div className="mb-8">
        <h1 className="text-2xl font-semibold text-text-base">{t("settings.shortcuts.title")}</h1>
      </div>

      <div className="mb-6 max-w-[800px]">
        <div className="relative">
          <input
            type="text"
            placeholder={t("settings.shortcuts.searchPlaceholder")}
            className="w-full bg-white border border-border-theme rounded-lg py-2.5 px-4 text-[13px] text-text-base focus:outline-none focus:border-blue-500 shadow-[0_1px_2px_rgb(0,0,0,0.02)]"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
      </div>

      <div className="max-w-[800px] border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white mb-20">
        <div className="flex px-4 py-3 border-b border-border-theme bg-white">
          <div className="flex-1 text-[13px] font-medium text-text-secondary">{t("settings.shortcuts.command")}</div>
          <div className="w-[300px] text-[13px] font-medium text-text-secondary">{t("settings.shortcuts.keybinding")}</div>
        </div>

        {filteredShortcuts.map((item, idx) => (
          <div
            key={idx}
            className="flex px-4 py-3 border-b border-border-theme hover:bg-black/5 transition-colors group"
          >
            <div className="flex-1 pr-4">
              <div className="text-[13px] text-text-base font-medium mb-0.5">
                {item.name}
              </div>
              <div className="text-[12px] text-text-secondary">{item.desc}</div>
            </div>
            <div className="w-[300px] flex flex-col justify-center space-y-2">
              {item.keys.length > 0 ? (
                item.keys.map((keybind, keyIdx) => (
                  <div key={keyIdx} className="flex items-center">
                    <span className="px-2.5 py-0.5 bg-gray-50 border border-gray-200 rounded-full text-[12px] text-gray-500 shadow-sm flex items-center h-6">
                      {keybind === "Enter" ? (
                        <span className="text-[14px]">↵</span>
                      ) : (
                        keybind
                      )}
                    </span>
                    <div className="flex items-center ml-auto opacity-0 group-hover:opacity-100 transition-opacity space-x-3 text-gray-400">
                      <button className="hover:text-text-base transition-colors">
                        <FontAwesomeIcon icon={["fas", "pen"]} className="text-[11px]" />
                      </button>
                      <button className="hover:text-red-500 transition-colors">
                        <FontAwesomeIcon icon={["fas", "trash"]} className="text-[12px]" />
                      </button>
                    </div>
                  </div>
                ))
              ) : (
                <div className="flex items-center">
                  <span className="text-[12px] text-gray-400 h-6 flex items-center">{t("settings.shortcuts.unassigned")}</span>
                  <div className="flex items-center ml-auto opacity-0 group-hover:opacity-100 transition-opacity text-gray-400">
                    <button className="hover:text-text-base transition-colors">
                      <FontAwesomeIcon icon={["fas", "pen"]} className="text-[11px]" />
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>
        ))}
        {filteredShortcuts.length === 0 && (
          <div className="px-4 py-8 text-center text-[13px] text-text-secondary">
            {t("settings.shortcuts.noShortcuts")}
          </div>
        )}
      </div>
    </>
  );
}
