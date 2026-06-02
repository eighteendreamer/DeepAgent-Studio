import { useState, useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { ChatMessage, MessagePart, TokenUsage, TimelineEntry, ApprovalRequest } from "../types";
import { Composer } from "./Composer";
import { ToolCallCard } from "./ToolCallCard";
import { ApprovalDialog } from "./ApprovalDialog";
import { FilesPlugin } from "./plugins/FilesPlugin";
import { SideChatPlugin } from "./plugins/SideChatPlugin";
import { BrowserPlugin } from "./plugins/BrowserPlugin";
import { TerminalPlugin } from "./plugins/TerminalPlugin";
import { BottomPanelIcon, SidebarRightIcon } from "./icons";
import { useTranslation } from "react-i18next";

interface Props {
  messages: ChatMessage[];
  onSend: (text: string) => void;
  /** Fork the current session into a new branch from its latest point. */
  onFork?: () => void;
  /** Rewind the current session to a timeline sequence (destructive). */
  onRewind?: (toSeq: number) => void;
  /** Export the current session transcript. */
  onExport?: (format: "markdown" | "json") => void;
  /** The session timeline, used to offer rewind anchors. */
  timeline?: TimelineEntry[];
  /** Head-of-queue tool-approval request to show floating above the composer. */
  approval?: ApprovalRequest | null;
  /** Total queued approvals (including the current one). */
  approvalQueueCount?: number;
  /** Resolve the current approval (allow / deny). */
  onApprovalDecision?: (req: ApprovalRequest, approved: boolean) => void;
  /** True while a run is streaming — disables the composer send button. */
  busy?: boolean;
  /** Stop the in-flight run (manual cancel). */
  onStop?: () => void;
  /** Current session is in read-only Plan mode. */
  planMode?: boolean;
}

export type PluginType = "none" | "files" | "chat" | "browser" | "terminal";

export type Tab = {
  id: string;
  type: PluginType;
  title: string;
  icon: IconProp;
};

export const TOOL_CARDS: { icon: IconProp; title: string; desc: string; type: PluginType }[] = [
  { icon: ["far", "folder-open"], title: "files", desc: "filesDesc", type: "files" },
  { icon: ["far", "comment-dots"], title: "chat", desc: "chatDesc", type: "chat" },
  { icon: ["fas", "globe"], title: "browser", desc: "browserDesc", type: "browser" },
  { icon: ["fas", "terminal"], title: "terminal", desc: "terminalDesc", type: "terminal" },
];

export function ChatView({ messages, onSend, onFork, onRewind, onExport, timeline = [], approval = null, approvalQueueCount = 0, onApprovalDecision, busy = false, onStop, planMode = false }: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [isOutputPanelOpen, setIsOutputPanelOpen] = useState(true);
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const [isBottomPanelOpen, setIsBottomPanelOpen] = useState(false);
  const [isRewindOpen, setIsRewindOpen] = useState(false);
  const [bottomTabs, setBottomTabs] = useState<Tab[]>([]);
  const [activeBottomTabId, setActiveBottomTabId] = useState<string>("new");

  const [isRightSidebarOpen, setIsRightSidebarOpen] = useState(false);
  const [rightSidebarWidth, setRightSidebarWidth] = useState(600);
  const [isResizingSidebar, setIsResizingSidebar] = useState(false);
  const [bottomPanelHeight, setBottomPanelHeight] = useState(280);
  const [isResizingBottom, setIsResizingBottom] = useState(false);

  useEffect(() => {
    if (!isResizingBottom) return;
    const handleMouseMove = (e: MouseEvent) => {
      const newHeight = window.innerHeight - e.clientY;
      if (newHeight > 200 && newHeight < window.innerHeight - 100) {
        setBottomPanelHeight(newHeight);
      }
    };
    const handleMouseUp = () => setIsResizingBottom(false);

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isResizingBottom]);

  useEffect(() => {
    if (!isResizingSidebar) return;
    const handleMouseMove = (e: MouseEvent) => {
      const newWidth = window.innerWidth - e.clientX;
      if (newWidth > 360 && newWidth < window.innerWidth - 100) {
        setRightSidebarWidth(newWidth);
      }
    };
    const handleMouseUp = () => setIsResizingSidebar(false);

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isResizingSidebar]);
  const [sidebarTabs, setSidebarTabs] = useState<Tab[]>([]);
  const [activeSidebarTabId, setActiveSidebarTabId] = useState<string>("new");

  // Auto-scroll the conversation to the bottom as messages/tokens stream in.
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const getTranslatedToolName = (_title: string, type: string) => {
    return t(`chatView.tools.${type}`);
  };

  const handleOpenBottomPlugin = (c: typeof TOOL_CARDS[0]) => {
    const newTab: Tab = {
      id: Date.now().toString(),
      type: c.type,
      title: c.title === "terminal" ? "C:\WINDOWS\System32\..." : 
             c.title === "files" ? "AUTH_SPEC.md" : getTranslatedToolName(c.title, c.type),
      icon: c.title === "terminal" ? ["fas", "terminal"] :
            c.title === "files" ? ["far", "file-lines"] : c.icon
    };
    setBottomTabs([...bottomTabs, newTab]);
    setActiveBottomTabId(newTab.id);
  };

  const handleOpenSidebarPlugin = (c: typeof TOOL_CARDS[0]) => {
    const newTab: Tab = {
      id: Date.now().toString(),
      type: c.type,
      title: c.title === "terminal" ? "C:\WINDOWS\System32\..." : 
             c.title === "files" ? "AUTH_SPEC.md" : getTranslatedToolName(c.title, c.type),
      icon: c.title === "terminal" ? ["fas", "terminal"] :
            c.title === "files" ? ["far", "file-lines"] : c.icon
    };
    setSidebarTabs([...sidebarTabs, newTab]);
    setActiveSidebarTabId(newTab.id);
  };

  const submit = () => {
    const t = value.trim();
    if (!t) return;
    onSend(t);
    setValue("");
  };

  return (
    <div className="w-full h-full flex flex-col relative">
      {/* Top half: conversation flow & overlay */}
      <div className="flex-1 flex flex-col h-full w-full relative overflow-hidden">
        <header className="h-14 flex items-center px-6 justify-between flex-shrink-0 w-full">
          <div className="relative">
            <div 
              className="flex items-center text-sm font-medium text-text-base cursor-pointer px-2 py-1 -ml-2 rounded hover:bg-gray-100 transition-colors"
              onClick={() => setIsMenuOpen(!isMenuOpen)}
            >
              {t("chatView.chat")}
              <FontAwesomeIcon
                icon={["fas", "ellipsis"]}
                className="ml-2 text-text-secondary"
              />
            </div>
            
            {/* Dropdown Menu */}
            {isMenuOpen && (
              <div className="absolute top-10 left-0 w-60 bg-white border border-border-theme rounded-xl shadow-lg py-1.5 z-50 text-[13px] text-text-base font-normal">
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group">
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "thumbtack"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.pinChat")}</span>
                  </div>
                  <span className="text-gray-400 text-[11px] font-sans">Ctrl+Alt+P</span>
                </div>
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group">
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "pen"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.renameChat")}</span>
                  </div>
                  <span className="text-gray-400 text-[11px] font-sans">Ctrl+Alt+R</span>
                </div>
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group">
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "box-archive"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.archiveChat")}</span>
                  </div>
                  <span className="text-gray-400 text-[11px] font-sans">Ctrl+Shift+A</span>
                </div>
                
                <div className="w-full h-px bg-border-theme my-1.5"></div>
                
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group">
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["far", "window-restore"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.openSideChat")}</span>
                  </div>
                </div>
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onExport?.("markdown");
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["far", "copy"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.copy")}</span>
                  </div>
                  <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-gray-400" />
                </div>
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onExport?.("json");
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "file-export"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.exportJson")}</span>
                  </div>
                </div>
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onFork?.();
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "code-branch"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.branch")}</span>
                  </div>
                  <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-gray-400" />
                </div>
                <div className="relative">
                  <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                    onClick={() => setIsRewindOpen((v) => !v)}
                  >
                    <div className="flex items-center">
                      <FontAwesomeIcon icon={["fas", "clock-rotate-left"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                      <span>{t("chatView.rewind")}</span>
                    </div>
                    <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-gray-400" />
                  </div>
                  {isRewindOpen && (
                    <div className="absolute left-full top-0 ml-1 w-64 max-h-72 overflow-y-auto bg-white border border-border-theme rounded-xl shadow-lg py-1.5 z-50 custom-scrollbar">
                      {timeline.length === 0 && (
                        <div className="px-4 py-2 text-[12px] text-text-secondary">{t("chatView.noRewindPoints")}</div>
                      )}
                      {timeline.map((entry) => (
                        <div
                          key={entry.sequence}
                          className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer text-[12px] text-text-base"
                          onClick={() => {
                            onRewind?.(entry.sequence);
                            setIsRewindOpen(false);
                            setIsMenuOpen(false);
                          }}
                          title={entry.detail ?? undefined}
                        >
                          <span className="text-gray-400 mr-2 tabular-nums">#{entry.sequence}</span>
                          <span className="truncate">{entry.label}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>

                <div className="w-full h-px bg-border-theme my-1.5"></div>

                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group">
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["far", "clock"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.addAutomation")}</span>
                  </div>
                </div>

                <div className="w-full h-px bg-border-theme my-1.5"></div>

                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group">
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.openInNewWindow")}</span>
                  </div>
                </div>
              </div>
            )}
          </div>
          <div className="flex items-center space-x-3 text-text-secondary">
            <FontAwesomeIcon 
              icon={["fas", "sliders"]} 
              className={`cursor-pointer transition-colors text-sm ${isOutputPanelOpen ? "text-text-base" : "hover:text-text-base"}`}
              onClick={() => setIsOutputPanelOpen(!isOutputPanelOpen)}
            />
            <BottomPanelIcon 
              className="cursor-pointer transition-colors hover:text-text-base"
              onClick={() => {
            if (isBottomPanelOpen) {
              setIsBottomPanelOpen(false);
            } else {
              setIsBottomPanelOpen(true);
              if (!bottomTabs.some(t => t.type === "terminal")) {
                const terminalCard = TOOL_CARDS.find(c => c.type === "terminal");
                if (terminalCard) handleOpenBottomPlugin(terminalCard);
              } else {
                const termTab = bottomTabs.find(t => t.type === "terminal");
                if (termTab) setActiveBottomTabId(termTab.id);
              }
            }
          }}
            />
            <SidebarRightIcon
              className={`cursor-pointer transition-colors ${isRightSidebarOpen ? "text-text-base" : "hover:text-text-base"}`}
              onClick={() => setIsRightSidebarOpen(!isRightSidebarOpen)}
            />
          </div>
        </header>
      <div className="flex-1 flex overflow-hidden relative">
        <div className="flex-1 flex flex-col relative min-w-0 min-h-0">
          

        <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto px-6 py-4 pb-44">
          {messages.length === 0 && (
            <div className="w-full max-w-4xl mx-auto text-text-secondary text-[15px] pl-2">
              {t("chatView.startConversation")}
            </div>
          )}
          {messages.map((m, i) =>
            m.role === "user" ? (
              <div key={i} className="flex flex-col items-end mb-8 w-full max-w-4xl mx-auto group">
                <div className="bg-gray-100 text-text-base px-4 py-2.5 rounded-2xl rounded-tr-sm text-[15px] max-w-[80%]">
                  {m.content}
                </div>
                <div className="flex text-text-secondary mt-2 space-x-3 text-sm opacity-0 group-hover:opacity-100 transition-opacity w-full justify-end">
                  <FontAwesomeIcon icon={["far", "copy"]} className="cursor-pointer hover:text-text-base" />
                  <FontAwesomeIcon icon={["fas", "pen"]} className="cursor-pointer hover:text-text-base" />
                </div>
              </div>
            ) : (
              <AssistantTurn
                key={i}
                message={m}
                busy={busy && i === messages.length - 1}
              />
            )
          )}
        </div>

        <div className="absolute bottom-6 left-0 w-full px-6 flex justify-center">
          <div className="w-full max-w-4xl">
            {approval && (
              <div className="mb-3">
                <ApprovalDialog
                  request={approval}
                  queueCount={approvalQueueCount}
                  onApprove={(req) => onApprovalDecision?.(req, true)}
                  onReject={(req) => onApprovalDecision?.(req, false)}
                />
              </div>
            )}
            <Composer
              value={value}
              onChange={setValue}
              onSubmit={submit}
              placeholder={t("chatView.requestFollowUp")}
              busy={busy}
              onStop={onStop}
              planMode={planMode}
            />
          </div>
        </div>

        
          
      {/* Bottom Panel */}
      {isBottomPanelOpen && (
        <div 
          className={`absolute left-0 right-0 bottom-0 bg-white flex flex-col z-50 shadow-[0_0_40px_rgba(0,0,0,0.1)] border-t border-border-theme transform translate-y-0 ${isResizingBottom ? "" : "transition-transform duration-300"}`}
          style={{ height: `${bottomPanelHeight}px`, minHeight: '200px', maxHeight: '80vh' }}
        >
          {/* Resize Handle */}
          <div 
            className="absolute left-0 right-0 top-0 h-1.5 cursor-row-resize hover:bg-blue-500/50 z-50 -mt-[1px]"
            onMouseDown={(e) => {
              e.preventDefault();
              setIsResizingBottom(true);
            }}
          />
          {/* Global Tab Bar */}
          {/* Resize Handle */}
          <div 
            className="absolute left-0 top-0 bottom-0 w-1.5 cursor-col-resize hover:bg-blue-500/50 z-50 -ml-[1px]"
            onMouseDown={(e) => {
              e.preventDefault();
              setIsResizingSidebar(true);
            }}
          />
          <div className="flex items-center justify-between border-b border-border-theme h-10 px-4 flex-shrink-0 bg-white">
            <div className="flex items-center text-[13px] text-text-secondary h-full">
              {bottomTabs.map(tab => (
                <div 
                  key={tab.id}
                  onClick={() => setActiveBottomTabId(tab.id)}
                  className={`flex items-center h-full px-3 border-b-2 cursor-pointer ${
                    activeBottomTabId === tab.id 
                      ? "border-text-base text-text-base" 
                      : "border-transparent hover:text-text-base"
                  }`}
                >
                  <FontAwesomeIcon icon={tab.icon} className="mr-2" />
                  {tab.title}
                  <FontAwesomeIcon 
                    icon={["fas", "xmark"]} 
                    className="ml-3 hover:text-red-500 text-[10px]" 
                    onClick={(e) => {
                       e.stopPropagation();
                       const newTabs = bottomTabs.filter(t => t.id !== tab.id);
                       setBottomTabs(newTabs);
                       if (activeBottomTabId === tab.id) {
                         setActiveBottomTabId(newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new");
                       }
                    }}
                  />
                </div>
              ))}
              <div 
                className={`flex items-center h-full px-3 cursor-pointer ${activeBottomTabId === "new" ? "text-text-base" : "hover:text-text-base"}`}
                onClick={() => setActiveBottomTabId("new")}
              >
                <FontAwesomeIcon icon={["fas", "plus"]} />
              </div>
            </div>
            <div className="flex items-center space-x-3 text-text-secondary">
              {bottomTabs.find(t => t.id === activeBottomTabId)?.type === "files" && (
                <>
                  <FontAwesomeIcon icon={["fas", "ellipsis"]} className="cursor-pointer hover:text-text-base" />
                  <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="cursor-pointer hover:text-text-base text-[13px]" />
                  <FontAwesomeIcon icon={["far", "copy"]} className="cursor-pointer hover:text-text-base" />
                </>
              )}
              <FontAwesomeIcon icon={["fas", "xmark"]} className="cursor-pointer hover:text-text-base ml-2" onClick={() => setIsBottomPanelOpen(false)} />
            </div>
          </div>

          <div className="flex-1 overflow-hidden flex flex-col relative">
            {activeBottomTabId === "new" && (
              <div className="w-full h-full flex flex-col relative">
                <div className="flex-1 flex items-center justify-center pt-8 pb-4">
                  <div className="flex space-x-4">
                    {TOOL_CARDS.map((c) => (
                      <div
                        key={c.title}
                        onClick={() => handleOpenBottomPlugin(c)}
                        className="bg-gray-50 rounded-2xl p-5 flex flex-col items-center justify-center text-center cursor-pointer hover:bg-gray-100 transition-colors h-[110px] w-[140px] border border-transparent hover:border-gray-200"
                      >
                        <FontAwesomeIcon icon={c.icon} className="text-[22px] text-text-base mb-2.5" />
                        <div className="text-[13px] font-medium text-text-base mb-1">{getTranslatedToolName(c.title, c.type)}</div>
                        <div className="text-[11px] text-text-secondary">{t(`chatView.tools.${c.type}Desc`)}</div>
                      </div>
                    ))}
                  </div>
                </div>

                <div className="pl-12 pb-4 text-[13px] text-text-secondary font-medium">{t("chatView.recommended")}</div>
              </div>
            )}

            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "files" && <FilesPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "chat" && <SideChatPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "browser" && <BrowserPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "terminal" && <TerminalPlugin />}
          </div>
        </div>
      )}
        </div>
        
      {/* Right Sidebar */}
      {isRightSidebarOpen && (
        <>
        
        {/* Drawer Panel */}
        <div 
          className={`absolute right-0 top-0 h-full bg-white flex flex-col z-50 shadow-[0_0_40px_rgba(0,0,0,0.1)] border-l border-border-theme transform translate-x-0 ${isResizingSidebar ? "" : "transition-transform duration-300"}`}
          style={{ width: `${rightSidebarWidth}px`, minWidth: '360px' }}
        >
          {/* Resize Handle */}
          <div 
            className="absolute left-0 top-0 bottom-0 w-1.5 cursor-col-resize hover:bg-blue-500/50 z-50 -ml-[1px]"
            onMouseDown={(e) => {
              e.preventDefault();
              setIsResizingSidebar(true);
            }}
          />
          <div className="flex items-center justify-between border-b border-border-theme h-10 px-4 flex-shrink-0 bg-white">
            <div className="flex items-center text-[13px] text-text-secondary h-full overflow-x-auto no-scrollbar">
              {sidebarTabs.map(tab => (
                <div 
                  key={tab.id}
                  onClick={() => setActiveSidebarTabId(tab.id)}
                  className={`flex items-center h-full px-3 border-b-2 cursor-pointer flex-shrink-0 ${
                    activeSidebarTabId === tab.id 
                      ? "border-text-base text-text-base" 
                      : "border-transparent hover:text-text-base"
                  }`}
                >
                  <FontAwesomeIcon icon={tab.icon} className="mr-2" />
                  {tab.title}
                  <FontAwesomeIcon 
                    icon={["fas", "xmark"]} 
                    className="ml-3 hover:text-red-500 text-[10px]" 
                    onClick={(e) => {
                       e.stopPropagation();
                       const newTabs = sidebarTabs.filter(t => t.id !== tab.id);
                       setSidebarTabs(newTabs);
                       if (activeSidebarTabId === tab.id) {
                         setActiveSidebarTabId(newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new");
                       }
                    }}
                  />
                </div>
              ))}
              <div 
                className={`flex items-center h-full px-3 cursor-pointer flex-shrink-0 ${activeSidebarTabId === "new" ? "text-text-base" : "hover:text-text-base"}`}
                onClick={() => setActiveSidebarTabId("new")}
              >
                <FontAwesomeIcon icon={["fas", "plus"]} />
              </div>
            </div>
            <div className="flex items-center space-x-3 text-text-secondary ml-2 flex-shrink-0 bg-white pl-2">
              {sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "files" && (
                <>
                  <FontAwesomeIcon icon={["fas", "ellipsis"]} className="cursor-pointer hover:text-text-base" />
                  <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="cursor-pointer hover:text-text-base text-[13px]" />
                  <FontAwesomeIcon icon={["far", "copy"]} className="cursor-pointer hover:text-text-base" />
                </>
              )}
              <FontAwesomeIcon icon={["fas", "xmark"]} className="cursor-pointer hover:text-text-base ml-1" onClick={() => setIsRightSidebarOpen(false)} />
            </div>
          </div>

          <div className="flex-1 overflow-hidden flex flex-col relative">
            {activeSidebarTabId === "new" && (
              <div className="w-full h-full flex flex-col relative overflow-y-auto bg-white">
                <div className="flex-1 flex flex-col p-6">
                  <div className="grid grid-cols-2 gap-4">
                    {TOOL_CARDS.map((c) => (
                      <div
                        key={c.title}
                        onClick={() => handleOpenSidebarPlugin(c)}
                        className="bg-gray-50 rounded-2xl p-4 flex flex-col items-center justify-center text-center cursor-pointer hover:bg-gray-100 transition-colors border border-transparent hover:border-gray-200 aspect-square"
                      >
                        <FontAwesomeIcon icon={c.icon} className="text-[20px] text-text-base mb-2" />
                        <div className="text-[12px] font-medium text-text-base mb-1">{getTranslatedToolName(c.title, c.type)}</div>
                        <div className="text-[10px] text-text-secondary leading-tight line-clamp-2">{t(`chatView.tools.${c.type}Desc`)}</div>
                      </div>
                    ))}
                  </div>
                </div>
                <div className="px-6 pb-6 text-[13px] text-text-secondary font-medium">{t("chatView.recommended")}</div>
              </div>
            )}

            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "files" && <FilesPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "chat" && <SideChatPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "browser" && <BrowserPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "terminal" && <TerminalPlugin />}
          </div>
        </div>
        </>
      )}
      </div>
      {/* Floating sticky note for Model Output State */}
        {isOutputPanelOpen && (
          <div className="absolute top-16 right-6 w-[280px] bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.08)] flex flex-col z-10 max-h-[calc(100%-120px)]">
            <div className="flex-1 flex flex-col p-5 overflow-y-auto">
              {/* Outputs */}
              <div className="mb-5">
                <div className="text-xs text-text-secondary mb-3">{t("chatView.output")}</div>
                <div className="space-y-2">
                  <div className="flex items-center text-[13px] text-text-base hover:text-blue-500 cursor-pointer transition-colors truncate">
                    <FontAwesomeIcon icon={["fas", "globe"]} className="w-4 mr-2 text-text-secondary" />
                    127.0.0.1:5005
                  </div>
                  <div className="flex items-center text-[13px] text-text-base hover:text-blue-500 cursor-pointer transition-colors truncate">
                    <FontAwesomeIcon icon={["fas", "globe"]} className="w-4 mr-2 text-text-secondary" />
                    localhost:5018/api/trinity/entitlemen...
                  </div>
                  <div className="flex items-center text-[13px] text-text-base hover:text-blue-500 cursor-pointer transition-colors truncate">
                    <FontAwesomeIcon icon={["far", "file-lines"]} className="w-4 mr-2 text-text-secondary" />
                    new_arch_spec.md
                  </div>
                  <div className="flex items-center text-[13px] text-text-base hover:text-blue-500 cursor-pointer transition-colors truncate">
                    <FontAwesomeIcon icon={["fas", "globe"]} className="w-4 mr-2 text-text-secondary" />
                    localhost:5018/api/trinity/entitlement
                  </div>
                  <div className="flex items-center text-[13px] text-text-base hover:text-blue-500 cursor-pointer transition-colors truncate">
                    <FontAwesomeIcon icon={["fas", "globe"]} className="w-4 mr-2 text-text-secondary" />
                    localhost:3100/xhc/
                  </div>
                </div>
              </div>

              <div className="w-full h-px bg-border-theme mb-5"></div>

              {/* Sources */}
              <div>
                <div className="text-xs text-text-secondary mb-3">{t("chatView.sources")}</div>
                <div className="text-[13px] text-text-secondary">{t("chatView.noSources")}</div>
              </div>
            </div>
          </div>
        )}
    </div>
    </div>
  );
}

/**
 * Wraps one assistant turn in a coherent block. A long agent run produces many
 * reasoning/tool "process" steps before the final answer; once the model starts
 * summarizing (final answer text appears) or the run finishes, those process
 * steps collapse into ONE big dropdown so the answer stays prominent. While the
 * run is still working with no answer yet, the steps stay expanded for live
 * progress.
 */
function AssistantTurn({ message: m, busy }: { message: ChatMessage; busy: boolean }) {
  const { t } = useTranslation();
  const parts = m.parts ?? [];

  // Split into process steps (reasoning + tools, plus any interleaved
  // intermediate text) and the final answer (the trailing run of text parts).
  let lastNonText = -1;
  for (let i = 0; i < parts.length; i++) {
    if (parts[i].kind !== "text") lastNonText = i;
  }
  const processParts = lastNonText >= 0 ? parts.slice(0, lastNonText + 1) : [];
  const answerParts = lastNonText >= 0 ? parts.slice(lastNonText + 1) : parts;

  const toolCount = processParts.filter((p) => p.kind === "tool").length;
  const hasProcess = processParts.length > 0 && toolCount > 0;
  const hasAnswer = answerParts.some(
    (p) => p.kind === "text" && p.text.trim().length > 0
  );

  // Total wall-clock spent in tool calls this turn (sum of card durations).
  const totalToolMs = parts.reduce(
    (acc, p) => acc + (p.kind === "tool" ? p.tool.durationMs ?? 0 : 0),
    0
  );

  // Collapse the process once the answer has begun (summary phase) or the run
  // finished; keep it open while actively working with nothing summarized yet.
  const collapseProcess = hasAnswer || !busy;

  const renderPart = (part: MessagePart, pi: number, streamTail: boolean) => {
    if (part.kind === "tool") {
      return <ToolCallCard key={`tool-${part.tool.call_id}`} tool={part.tool} />;
    }
    if (part.kind === "reasoning") {
      return <ReasoningBlock key={`r-${pi}`} text={part.text} defaultOpen={streamTail} />;
    }
    return (
      <div
        key={`t-${pi}`}
        className={`text-[15px] leading-relaxed whitespace-pre-wrap mb-1 ${
          part.tone === "error" ? "text-red-500" : "text-text-base"
        }`}
      >
        {part.text}
      </div>
    );
  };

  return (
    <div className="mb-6 w-full max-w-4xl mx-auto">
      {/* Header: agent label + live status. */}
      <div className="flex items-center mb-2 pl-1">
        <div className="w-5 h-5 rounded-md bg-primary/10 text-primary flex items-center justify-center mr-2">
          <FontAwesomeIcon icon={["fas", "robot"]} className="text-[11px]" />
        </div>
        <span className="text-[12px] font-medium text-text-base">{t("chatView.assistant")}</span>
        {busy && (
          <span className="ml-2 flex items-center text-[11px] text-blue-500">
            <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-1 text-[10px]" />
            {t("chatView.working")}
          </span>
        )}
      </div>

      {parts.length > 0 ? (
        <>
          {/* Process steps: collapsed into one dropdown once summarizing. */}
          {hasProcess &&
            (collapseProcess ? (
              <ProcessSteps toolCount={toolCount} totalMs={totalToolMs}>
                {processParts.map((p, pi) => renderPart(p, pi, false))}
              </ProcessSteps>
            ) : (
              <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3 mb-2">
                {processParts.map((p, pi) =>
                  renderPart(p, pi, busy && pi === processParts.length - 1 && !hasAnswer)
                )}
              </div>
            ))}

          {/* Final answer: always prominent, outside the collapsible. */}
          {hasAnswer && (
            <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3">
              {answerParts.map((p, pi) => renderPart(p, processParts.length + pi, false))}
              {!busy && (
                <UsageFooter usage={m.usage} totalMs={m.runMs ?? totalToolMs} answer={m.content} />
              )}
            </div>
          )}

          {/* Working placeholder before any step/answer exists. */}
          {busy && !hasProcess && !hasAnswer && (
            <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3">
              <div className="flex items-center text-text-secondary text-[14px]">
                <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2 text-[13px]" />
                {t("chatView.working")}
              </div>
            </div>
          )}
        </>
      ) : (
        // Legacy layout (replayed sessions without ordered parts).
        <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3">
          {m.tools && m.tools.length > 0 && (
            <ProcessSteps toolCount={m.tools.length} totalMs={0}>
              {m.tools.map((tool) => (
                <ToolCallCard key={tool.call_id} tool={tool} />
              ))}
            </ProcessSteps>
          )}
          {m.reasoning && <ReasoningBlock text={m.reasoning} />}
          {m.content ? (
            <div
              className={`text-[15px] leading-relaxed whitespace-pre-wrap ${
                m.tone === "error" ? "text-red-500" : "text-text-base"
              }`}
            >
              {m.content}
            </div>
          ) : (
            busy && (
              <div className="flex items-center text-text-secondary text-[14px]">
                <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2 text-[13px]" />
                {t("chatView.working")}
              </div>
            )
          )}
          {!busy && m.content && (
            <UsageFooter usage={m.usage} totalMs={m.runMs ?? 0} answer={m.content} />
          )}
        </div>
      )}
    </div>
  );
}

/**
 * A big collapsible container that folds away a long run's process steps
 * (reasoning + tool calls), keeping the final answer prominent. Collapsed by
 * default; the header summarizes how many tools ran.
 */
function ProcessSteps({
  toolCount,
  totalMs,
  children,
}: {
  toolCount: number;
  totalMs: number;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <div className="w-full mb-2 border border-border-theme rounded-xl bg-gray-50/60 overflow-hidden">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center px-4 py-2.5 text-[13px] text-text-secondary hover:text-text-base transition-colors"
      >
        <FontAwesomeIcon
          icon={["fas", open ? "chevron-down" : "chevron-right"]}
          className="mr-2 text-[11px]"
        />
        <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-2 text-[12px]" />
        <span className="font-medium">{t("chatView.processSteps", { count: toolCount })}</span>
        {totalMs > 0 && (
          <span className="ml-2 text-[11px] text-text-secondary tabular-nums">· {formatMs(totalMs)}</span>
        )}
        {!open && <span className="ml-2 text-[11px] text-text-secondary">{t("chatView.clickToExpand")}</span>}
      </button>
      {open && <div className="border-t border-border-theme px-4 py-3">{children}</div>}
    </div>
  );
}

/** Format a millisecond duration compactly (e.g. 850ms, 2.3s, 1m12s). */
function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rem = Math.round(s % 60);
  return `${m}m${rem}s`;
}

/** Compact number (1234 → 1.2k). */
function formatTokens(n: number): string {
  if (n < 1000) return `${n}`;
  return `${(n / 1000).toFixed(n < 10000 ? 1 : 0)}k`;
}

/**
 * Estimate this turn's spend (¥) from the token breakdown, using DeepSeek-chat
 * default pricing (input ¥1/M, output ¥2/M, cache-hit ¥0.1/M). This mirrors the
 * backend's `default_pricing` for the common chat model; the authoritative
 * cumulative figure lives in the cost summary. Cache-hit tokens are a subset of
 * input billed at the discounted rate.
 */
function estimateCostYuan(usage: TokenUsage): number {
  const fullInput = Math.max(0, usage.promptTokens - usage.cacheHitTokens);
  const yuan =
    (fullInput * 1.0 + usage.completionTokens * 2.0 + usage.cacheHitTokens * 0.1) / 1_000_000;
  return Math.round(yuan * 10000) / 10000;
}

/** Format a ¥ amount with adaptive precision (small spends keep 4 decimals). */
function formatYuan(n: number): string {
  if (n <= 0) return "¥0";
  if (n < 0.01) return `¥${n.toFixed(4)}`;
  if (n < 1) return `¥${n.toFixed(3)}`;
  return `¥${n.toFixed(2)}`;
}

/**
 * The footer shown under a finished assistant answer: a token/usage metaline
 * (total + input/output breakdown + cache hit) and total duration, plus a row
 * of action buttons (copy now; worktree and others wired in later). Gives the
 * answer breathing room from the composer and a home for per-turn actions.
 */
function UsageFooter({
  usage,
  totalMs: durationMs,
  answer,
}: {
  usage?: TokenUsage;
  totalMs: number;
  answer?: string;
}) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const onCopy = () => {
    if (!answer) return;
    navigator.clipboard?.writeText(answer).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      },
      () => {}
    );
  };

  const hasMetrics = !!usage || durationMs > 0;

  return (
    <div className="mt-3 pt-2.5 border-t border-border-theme">
      {/* Metrics line(s) */}
      {hasMetrics && (
        <div className="flex flex-col gap-0.5 text-[11px] text-text-secondary tabular-nums">
          {usage && (
            <div className="flex items-center flex-wrap gap-x-1.5">
              <span className="font-medium text-text-base">
                {formatTokens(usage.totalTokens)} tokens
              </span>
              <span className="text-text-secondary">
                ({formatTokens(usage.promptTokens)}
                <FontAwesomeIcon icon={["fas", "arrow-down"]} className="mx-0.5 text-[9px]" />
                {formatTokens(usage.completionTokens)}
                <FontAwesomeIcon icon={["fas", "arrow-up"]} className="ml-0.5 text-[9px]" />)
              </span>
              {usage.cacheHitTokens > 0 && (
                <span className="text-green-600 ml-1" title={t("chatView.tokensCacheHit")}>
                  <FontAwesomeIcon icon={["fas", "bolt"]} className="mr-0.5 text-[9px]" />
                  {t("chatView.tokensCacheHit")} {formatTokens(usage.cacheHitTokens)}
                </span>
              )}
              <span
                className="text-text-secondary ml-1"
                title={t("chatView.turnCost")}
              >
                <FontAwesomeIcon icon={["fas", "coins"]} className="mr-0.5 text-[9px]" />
                {formatYuan(estimateCostYuan(usage))}
              </span>
            </div>
          )}
          {durationMs > 0 && (
            <div className="flex items-center">
              <FontAwesomeIcon icon={["far", "clock"]} className="mr-1 text-[9px]" />
              {t("chatView.totalDuration")}: {formatMs(durationMs)}
            </div>
          )}
        </div>
      )}

      {/* Actions row (copy now; worktree & more wired later). */}
      <div className="flex items-center gap-1 mt-1.5 -ml-1">
        <button
          onClick={onCopy}
          title={t("chatView.copyAnswer")}
          className="w-7 h-7 rounded-md flex items-center justify-center text-text-secondary hover:bg-gray-100 hover:text-text-base transition-colors"
        >
          <FontAwesomeIcon icon={copied ? ["fas", "check"] : ["far", "copy"]} className="text-[12px]" />
        </button>
        <button
          title={t("chatView.addWorktree")}
          className="w-7 h-7 rounded-md flex items-center justify-center text-text-secondary hover:bg-gray-100 hover:text-text-base transition-colors"
        >
          <FontAwesomeIcon icon={["fas", "code-branch"]} className="text-[12px]" />
        </button>
      </div>
    </div>
  );
}

/**
 * A collapsible block showing the model's full Thinking-Mode reasoning trace.
 * Auto-expands while it is the actively-streaming tail (so the user watches the
 * reasoning grow); collapsible afterwards to keep the answer prominent.
 */
function ReasoningBlock({ text, defaultOpen = false }: { text: string; defaultOpen?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(defaultOpen);
  // Follow the streaming state: open while streaming, collapse once it ends.
  const prevDefault = useRef(defaultOpen);
  useEffect(() => {
    if (prevDefault.current !== defaultOpen) {
      setOpen(defaultOpen);
      prevDefault.current = defaultOpen;
    }
  }, [defaultOpen]);
  return (
    <div className="w-full mb-3 border border-border-theme rounded-lg bg-gray-50/60 overflow-hidden">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center px-3 py-2 text-[12px] text-text-secondary hover:text-text-base transition-colors"
      >
        <FontAwesomeIcon
          icon={["fas", open ? "chevron-down" : "chevron-right"]}
          className="mr-2 text-[10px]"
        />
        <FontAwesomeIcon icon={["fas", "lightbulb"]} className="mr-1.5 text-[11px]" />
        {t("chatView.reasoning")}
      </button>
      {open && (
        <pre className="px-3 pb-3 text-[12px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed border-t border-border-theme pt-2">
          {text}
        </pre>
      )}
    </div>
  );
}
