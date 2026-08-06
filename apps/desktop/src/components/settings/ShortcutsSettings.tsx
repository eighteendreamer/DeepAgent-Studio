import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { useState } from "react";

type Shortcut = {
  name: string;
  desc: string;
  keys: string[];
};

const getShortcutsData = (t: any): Shortcut[] => [
  { name: t("settings.shortcuts.archiveChat"), desc: t("settings.shortcuts.archiveChatDesc"), keys: ["Ctrl+Shift+A"] },
  { name: t("settings.shortcuts.newChat"), desc: t("settings.shortcuts.newChatDesc"), keys: ["Ctrl+N", "Ctrl+Shift+O"] },
  { name: t("settings.shortcuts.openSideChat"), desc: t("settings.shortcuts.openSideChatDesc"), keys: [] },
  { name: t("settings.shortcuts.openNewWindow"), desc: t("settings.shortcuts.openNewWindowDesc"), keys: [] },
  { name: t("settings.shortcuts.newQuickChat"), desc: t("settings.shortcuts.newQuickChatDesc"), keys: ["Ctrl+Alt+N"] },
  { name: t("settings.shortcuts.togglePin"), desc: t("settings.shortcuts.togglePinDesc"), keys: ["Ctrl+Alt+P"] },
  { name: t("settings.shortcuts.search"), desc: t("settings.shortcuts.searchDesc"), keys: ["Ctrl+F"] },
  { name: t("settings.shortcuts.focusBrowserAddress"), desc: t("settings.shortcuts.focusBrowserAddressDesc"), keys: ["Ctrl+L"] },
  { name: t("settings.shortcuts.goBack"), desc: t("settings.shortcuts.goBackDesc"), keys: ["Ctrl+["] },
  { name: t("settings.shortcuts.goForward"), desc: t("settings.shortcuts.goForwardDesc"), keys: ["Ctrl+]"] },
  { name: t("settings.shortcuts.nextChat"), desc: t("settings.shortcuts.nextChatDesc"), keys: ["Ctrl+Shift+]"] },
  { name: t("settings.shortcuts.prevChat"), desc: t("settings.shortcuts.prevChatDesc"), keys: ["Ctrl+Shift+["] },
  { name: t("settings.shortcuts.openBrowserTab"), desc: t("settings.shortcuts.openBrowserTabDesc"), keys: ["Ctrl+T"] },
  { name: t("settings.shortcuts.openReviewTab"), desc: t("settings.shortcuts.openReviewTabDesc"), keys: [] },
  { name: t("settings.shortcuts.toggleBrowserPanel"), desc: t("settings.shortcuts.toggleBrowserPanelDesc"), keys: ["Ctrl+Shift+B"] },
  { name: t("settings.shortcuts.toggleSidebar"), desc: t("settings.shortcuts.toggleSidebarDesc"), keys: ["Ctrl+B"] },
  { name: t("settings.shortcuts.toggleSidePanel"), desc: t("settings.shortcuts.toggleSidePanelDesc"), keys: ["Ctrl+Alt+B"] },
  { name: t("settings.shortcuts.toggleTerminal"), desc: t("settings.shortcuts.toggleTerminalDesc"), keys: ["Ctrl+J"] },
  { name: t("settings.shortcuts.openFolder"), desc: t("settings.shortcuts.openFolderDesc"), keys: ["Ctrl+O"] },
  { name: t("settings.shortcuts.forceReloadSkill"), desc: t("settings.shortcuts.forceReloadSkillDesc"), keys: [] },
  { name: t("settings.shortcuts.goToSkill"), desc: t("settings.shortcuts.goToSkillDesc"), keys: [] },
  { name: t("settings.shortcuts.installCodexWorkspace"), desc: t("settings.shortcuts.installCodexWorkspaceDesc"), keys: [] },
  { name: t("settings.shortcuts.keyboardShortcuts"), desc: t("settings.shortcuts.keyboardShortcutsDesc"), keys: [] },
  { name: t("settings.shortcuts.mcp"), desc: t("settings.shortcuts.mcpDesc"), keys: [] },
  { name: t("settings.shortcuts.personality"), desc: t("settings.shortcuts.personalityDesc"), keys: [] },
  { name: t("settings.shortcuts.feedback"), desc: t("settings.shortcuts.feedbackDesc"), keys: [] },
  { name: t("settings.shortcuts.signOut"), desc: t("settings.shortcuts.signOutDesc"), keys: [] },
  { name: t("settings.shortcuts.manageAutomations"), desc: t("settings.shortcuts.manageAutomationsDesc"), keys: [] },
  { name: t("settings.shortcuts.wakePet"), desc: t("settings.shortcuts.wakePetDesc"), keys: [] },
  { name: t("settings.shortcuts.openControlWindow"), desc: t("settings.shortcuts.openControlWindowDesc"), keys: [] },
  { name: t("settings.shortcuts.settings"), desc: t("settings.shortcuts.settingsDesc"), keys: ["Ctrl+,"] },
  { name: t("settings.shortcuts.approveRequest"), desc: t("settings.shortcuts.approveRequestDesc"), keys: ["Enter"] },
  { name: t("settings.shortcuts.declineRequest"), desc: t("settings.shortcuts.declineRequestDesc"), keys: ["Escape"] },
  { name: t("settings.shortcuts.close"), desc: t("settings.shortcuts.closeDesc"), keys: ["Ctrl+W"] },
  { name: t("settings.shortcuts.openModelPicker"), desc: t("settings.shortcuts.openModelPickerDesc"), keys: ["Ctrl+Shift+M"] },
  { name: t("settings.shortcuts.startDictation"), desc: t("settings.shortcuts.startDictationDesc"), keys: ["Ctrl+Shift+D"] },
  { name: t("settings.shortcuts.toggleVoiceMode"), desc: t("settings.shortcuts.toggleVoiceModeDesc"), keys: ["Ctrl+Shift+V"] },
  { name: t("settings.shortcuts.copyAsMarkdown"), desc: t("settings.shortcuts.copyAsMarkdownDesc"), keys: [] },
  { name: t("settings.shortcuts.copyConversationPath"), desc: t("settings.shortcuts.copyConversationPathDesc"), keys: ["Ctrl+Alt+Shift+C"] },
  { name: t("settings.shortcuts.copyDeeplink"), desc: t("settings.shortcuts.copyDeeplinkDesc"), keys: ["Ctrl+Alt+L"] },
  { name: t("settings.shortcuts.copySessionId"), desc: t("settings.shortcuts.copySessionIdDesc"), keys: ["Ctrl+Alt+C"] },
  { name: t("settings.shortcuts.copyWorkingDirectory"), desc: t("settings.shortcuts.copyWorkingDirectoryDesc"), keys: ["Ctrl+Shift+C"] },
  { name: t("settings.shortcuts.holdDictationShortcut"), desc: t("settings.shortcuts.holdDictationShortcutDesc"), keys: [] },
  { name: t("settings.shortcuts.toggleDictationShortcut"), desc: t("settings.shortcuts.toggleDictationShortcutDesc"), keys: [] },
  { name: t("settings.shortcuts.forceReloadBrowserPage"), desc: t("settings.shortcuts.forceReloadBrowserPageDesc"), keys: ["Ctrl+Shift+R"] },
  { name: t("settings.shortcuts.popupShortcut"), desc: t("settings.shortcuts.popupShortcutDesc"), keys: [] },
  { name: t("settings.shortcuts.newWindow"), desc: t("settings.shortcuts.newWindowDesc"), keys: ["Ctrl+Shift+N"] },
  { name: t("settings.shortcuts.nextRecentChat"), desc: t("settings.shortcuts.nextRecentChatDesc"), keys: ["Ctrl+Tab"] },
  { name: t("settings.shortcuts.openCommandMenu"), desc: t("settings.shortcuts.openCommandMenuDesc"), keys: ["Ctrl+K", "Ctrl+Shift+P"] },
  { name: t("settings.shortcuts.prevRecentChat"), desc: t("settings.shortcuts.prevRecentChatDesc"), keys: ["Ctrl+Shift+Tab"] },
  { name: t("settings.shortcuts.reloadBrowserPage"), desc: t("settings.shortcuts.reloadBrowserPageDesc"), keys: ["Ctrl+R"] },
  { name: t("settings.shortcuts.renameChat"), desc: t("settings.shortcuts.renameChatDesc"), keys: ["Ctrl+Alt+R"] },
  { name: t("settings.shortcuts.searchChats"), desc: t("settings.shortcuts.searchChatsDesc"), keys: ["Ctrl+G"] },
  { name: t("settings.shortcuts.searchFiles"), desc: t("settings.shortcuts.searchFilesDesc"), keys: ["Ctrl+P"] },
  { name: t("settings.shortcuts.showShortcuts"), desc: t("settings.shortcuts.showShortcutsDesc"), keys: ["Ctrl+Shift+/"] },
  { name: t("settings.shortcuts.goToChat1"), desc: t("settings.shortcuts.goToChat1Desc"), keys: ["Ctrl+1"] },
  { name: t("settings.shortcuts.goToChat2"), desc: t("settings.shortcuts.goToChat2Desc"), keys: ["Ctrl+2"] },
  { name: t("settings.shortcuts.goToChat3"), desc: t("settings.shortcuts.goToChat3Desc"), keys: ["Ctrl+3"] },
  { name: t("settings.shortcuts.goToChat4"), desc: t("settings.shortcuts.goToChat4Desc"), keys: ["Ctrl+4"] },
  { name: t("settings.shortcuts.goToChat5"), desc: t("settings.shortcuts.goToChat5Desc"), keys: ["Ctrl+5"] },
  { name: t("settings.shortcuts.goToChat6"), desc: t("settings.shortcuts.goToChat6Desc"), keys: ["Ctrl+6"] },
  { name: t("settings.shortcuts.goToChat7"), desc: t("settings.shortcuts.goToChat7Desc"), keys: ["Ctrl+7"] },
  { name: t("settings.shortcuts.goToChat8"), desc: t("settings.shortcuts.goToChat8Desc"), keys: ["Ctrl+8"] },
  { name: t("settings.shortcuts.goToChat9"), desc: t("settings.shortcuts.goToChat9Desc"), keys: ["Ctrl+9"] },
  { name: t("settings.shortcuts.toggleFileTree"), desc: t("settings.shortcuts.toggleFileTreeDesc"), keys: ["Ctrl+Shift+E"] },
  { name: t("settings.shortcuts.startTraceRecording"), desc: t("settings.shortcuts.startTraceRecordingDesc"), keys: ["Ctrl+Shift+5"] },
];

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
                        <FontAwesomeIcon icon={["fas", "trash-alt"]} className="text-[12px]" />
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
