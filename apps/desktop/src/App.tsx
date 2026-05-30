import { useCallback, useEffect, useMemo, useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import {
  getSessionDetail,
  listSessions,
  listProjects,
  getActiveProject,
  setActiveProject,
  addProject,
  pickProjectFolder,
  removeProject,
  clearApiKey,
  runChat,
  resolveApproval,
  forkSession,
  rewindSession,
  exportTranscript,
} from "./api";
import type { RuntimeEvent } from "./api";
import type {
  ApprovalRequest,
  ChatMessage,
  Project,
  SessionDetail,
  SessionSummary,
} from "./types";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { StartView } from "./components/StartView";
import { ChatView } from "./components/ChatView";
import { SearchModal } from "./components/SearchModal";
import { ApprovalDialog } from "./components/ApprovalDialog";
import { SkillsView } from "./components/SkillsView";
import { AutomationView } from "./components/AutomationView";
import { OnboardingWizard } from "./components/OnboardingWizard";
import { SettingsSidebar } from "./components/SettingsSidebar";
import { SettingsView } from "./components/SettingsView";
import { message } from "./components/message";

type View = "start" | "chat" | "skills" | "automation" | "settings";

const SUGGESTIONS = [
  "Fix XHC Trinity production config before the next server deploy",
  "Sync the new prompt packs into XHC's admin prompt manager",
  "Make Trinity quota charging consistent after yesterday's call-logic reset",
];

export function App() {

  const [showOnboarding, setShowOnboarding] = useState(
    () => !localStorage.getItem("onboarding_complete")
  );
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [activeProjectPath, setActiveProjectPath] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [view, setView] = useState<View>("start");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  // FIFO queue of tool-approval requests awaiting the user's decision.
  const [approvals, setApprovals] = useState<ApprovalRequest[]>([]);

  const [navState, setNavState] = useState({
    history: [] as { activeId: string | null; view: View }[],
    index: -1,
  });

  // Load projects + sessions + the active project on startup.
  useEffect(() => {
    listProjects().then(setProjects).catch(() => setProjects([]));
    getActiveProject().then(setActiveProjectPath).catch(() => {});
    listSessions()
      .then((s) => {
        setSessions(s);
        if (s.length > 0) {
          const initialId = s[0].id;
          setActiveId(initialId);
          setNavState({
            history: [{ activeId: initialId, view: "start" }],
            index: 0,
          });
        }
      })
      .catch(() => setSessions([]));
  }, []);

  useEffect(() => {
    if (!activeId) {
      setDetail(null);
      return;
    }
    getSessionDetail(activeId)
      .then(setDetail)
      .catch(() => setDetail(null));
  }, [activeId]);

  // The active project's display name (folder name), for the StartView header.
  const activeProjectName = useMemo(() => {
    const p = projects.find((p) => p.path === activeProjectPath);
    return p?.name ?? projects[0]?.name ?? "";
  }, [projects, activeProjectPath]);

  const timelineMessages = useMemo<ChatMessage[]>(() => {
    if (!detail) return [];
    return detail.timeline.map((t) => ({
      role: "assistant" as const,
      content: `${t.icon} ${t.label}${t.detail ? `\n${t.detail}` : ""}`,
    }));
  }, [detail]);

  const navigateTo = useCallback((newActiveId: string | null, newView: View) => {
    setActiveId(newActiveId);
    setView(newView);
    setMessages([]);
    setNavState((prev) => {
      const newHistory = prev.history.slice(0, prev.index + 1);
      newHistory.push({ activeId: newActiveId, view: newView });
      return { history: newHistory, index: newHistory.length - 1 };
    });
  }, []);

  const goBack = useCallback(() => {
    setNavState((prev) => {
      if (prev.index > 0) {
        const item = prev.history[prev.index - 1];
        setActiveId(item.activeId);
        setView(item.view);
        setMessages([]);
        return { ...prev, index: prev.index - 1 };
      }
      
      // Fallback: if there's no history to go back to, just return to the start view
      setActiveId(null);
      setView("start");
      setMessages([]);
      return prev;
    });
  }, []);

  const goForward = useCallback(() => {
    setNavState((prev) => {
      if (prev.index < prev.history.length - 1) {
        const item = prev.history[prev.index + 1];
        setActiveId(item.activeId);
        setView(item.view);
        setMessages([]);
        return { ...prev, index: prev.index + 1 };
      }
      return prev;
    });
  }, []);

  const onSelect = useCallback(
    (id: string) => {
      navigateTo(id, "chat");
    },
    [navigateTo]
  );

  const onNewChat = useCallback(() => {
    navigateTo(activeId, "start");
  }, [activeId, navigateTo]);

  // Reload the sessions + projects lists (after a run/fork changes them).
  const refreshSessions = useCallback(() => {
    listSessions()
      .then(setSessions)
      .catch(() => {});
    listProjects()
      .then(setProjects)
      .catch(() => {});
  }, []);

  // Switch the active project (agent ops + new sessions attach here).
  const onSelectProject = useCallback((path: string) => {
    setActiveProjectPath(path);
    setActiveProject(path).catch(() => {});
  }, []);

  // Open the native folder picker, then open the chosen folder as a project,
  // make it active, and refresh the project list.
  const onAddProject = useCallback(() => {
    pickProjectFolder()
      .then((path) => {
        if (!path) return;
        return addProject(path).then((p) => {
          setActiveProjectPath(p.path);
          return listProjects().then(setProjects);
        });
      })
      .catch(() => {});
  }, []);

  // Close (remove) a project from the sidebar; its sessions stay in the DB.
  // Refresh the project list and the active-project pointer afterwards.
  const onRemoveProject = useCallback((path: string) => {
    removeProject(path)
      .then(() => Promise.all([listProjects(), getActiveProject()]))
      .then(([ps, active]) => {
        setProjects(ps);
        setActiveProjectPath(active);
      })
      .catch(() => {});
  }, []);

  // Log out: delete the stored API key from the keychain, clear the local
  // onboarding flag, and return to the login (onboarding) screen.
  const onLogout = useCallback(() => {
    clearApiKey()
      .catch(() => {})
      .finally(() => {
        localStorage.removeItem("onboarding_complete");
        setShowOnboarding(true);
        message.success("已退出登录");
      });
  }, []);

  const onSubmit = useCallback(
    (text: string) => {
      if (!text) return;

      // Show the user's message immediately, plus an empty assistant message
      // we stream tokens into.
      setMessages([
        { role: "user", content: text },
        { role: "assistant", content: "" },
      ]);

      if (view !== "chat") {
        setView("chat");
        setNavState((prev) => {
          const newHistory = prev.history.slice(0, prev.index + 1);
          newHistory.push({ activeId, view: "chat" });
          return { history: newHistory, index: newHistory.length - 1 };
        });
      }

      // Append text to the (last) assistant message as deltas arrive.
      const appendAssistant = (delta: string) => {
        setMessages((prev) => {
          const next = [...prev];
          const lastIdx = next.length - 1;
          if (lastIdx >= 0 && next[lastIdx].role === "assistant") {
            next[lastIdx] = {
              ...next[lastIdx],
              content: next[lastIdx].content + delta,
            };
          }
          return next;
        });
      };

      const onEvent = (event: RuntimeEvent) => {
        switch (event.type) {
          case "content_delta":
            appendAssistant(String(event.text ?? ""));
            break;
          case "tool_started":
            appendAssistant(`\n\n🔧 ${String(event.name ?? "tool")}…`);
            break;
          case "tool_completed":
            appendAssistant(event.ok ? " ✓" : " ✗");
            break;
          case "tool_blocked":
            appendAssistant(
              `\n\n⛔ ${String(event.name ?? "tool")} blocked: ${String(event.reason ?? "")}`
            );
            break;
          case "run_failed":
            setMessages((prev) => {
              const next = [...prev];
              const lastIdx = next.length - 1;
              if (lastIdx >= 0 && next[lastIdx].role === "assistant") {
                next[lastIdx] = {
                  ...next[lastIdx],
                  content:
                    next[lastIdx].content ||
                    `run failed: ${String(event.reason ?? "unknown error")}`,
                  tone: "error",
                };
              }
              return next;
            });
            break;
          default:
            break;
        }
      };

      runChat(
        text,
        onEvent,
        (request) => {
          // Queue the approval request; the dialog shows the head of the queue.
          setApprovals((prev) => [...prev, request]);
        }
      )
        .then((newSessionId) => {
          // The run created a new session under the active project; refresh the
          // sidebar lists and focus the new session.
          refreshSessions();
          if (newSessionId) {
            setActiveId(newSessionId);
            setNavState((prev) => {
              const newHistory = prev.history.slice(0, prev.index + 1);
              newHistory.push({ activeId: newSessionId, view: "chat" });
              return { history: newHistory, index: newHistory.length - 1 };
            });
          }
        })
        .catch((err) => {
          setMessages((prev) => {
            const next = [...prev];
            const lastIdx = next.length - 1;
            if (lastIdx >= 0 && next[lastIdx].role === "assistant") {
              next[lastIdx] = {
                role: "assistant",
                content: `error: ${String(err)}`,
                tone: "error",
              };
            }
            return next;
          });
        });
    },
    [activeId, view, refreshSessions]
  );

  const chatMessages = messages.length > 0 ? messages : timelineMessages;

  // Resolve the head-of-queue approval (allow/deny), then dequeue it.
  const onApprovalDecision = useCallback(
    (request: ApprovalRequest, approved: boolean) => {
      resolveApproval(request.call_id, approved).catch(() => {});
      setApprovals((prev) => prev.filter((a) => a.call_id !== request.call_id));
    },
    []
  );

  // Fork the active session at its latest sequence into a new branch, then
  // switch to it.
  const onForkSession = useCallback(() => {
    if (!activeId || !detail) return;
    const lastSeq =
      detail.timeline.length > 0
        ? detail.timeline[detail.timeline.length - 1].sequence
        : 0;
    forkSession(activeId, lastSeq)
      .then((res) => {
        refreshSessions();
        navigateTo(res.new_session_id, "chat");
      })
      .catch(() => {});
  }, [activeId, detail, refreshSessions, navigateTo]);

  // Rewind the active session to a timeline sequence (destructive truncate),
  // then reload its detail.
  const onRewindSession = useCallback(
    (toSeq: number) => {
      if (!activeId) return;
      rewindSession(activeId, toSeq)
        .then(() => {
          setMessages([]);
          return getSessionDetail(activeId);
        })
        .then(setDetail)
        .catch(() => {});
    },
    [activeId]
  );

  // Export the active session transcript and trigger a browser/Tauri download.
  const onExportSession = useCallback(
    (format: "markdown" | "json") => {
      if (!activeId) return;
      exportTranscript(activeId, format)
        .then((t) => {
          const blob = new Blob([t.content], {
            type: format === "json" ? "application/json" : "text/markdown",
          });
          const url = URL.createObjectURL(blob);
          const a = document.createElement("a");
          a.href = url;
          a.download = `session-${activeId}.${t.extension}`;
          a.click();
          URL.revokeObjectURL(url);
        })
        .catch(() => {});
    },
    [activeId]
  );

  const [isSidebarOpen, setIsSidebarOpen] = useState(true);
  const [isSearchOpen, setIsSearchOpen] = useState(false);

  // Settings State
  const [activeSettingsCategory, setActiveSettingsCategory] = useState("general");

  const canGoBack = navState.index > 0;
  const canGoForward = navState.index < navState.history.length - 1;

  return (
    <div className="bg-sidebar-bg text-text-base font-sans h-screen w-full overflow-hidden flex flex-col relative">
      <TitleBar 
        onToggleSidebar={() => setIsSidebarOpen(!isSidebarOpen)} 
        isSidebarOpen={isSidebarOpen} 
        canGoBack={canGoBack}
        canGoForward={canGoForward}
        onBack={goBack}
        onForward={goForward}
      />

      <div className="flex-1 flex overflow-hidden">
        <AnimatePresence mode="wait">
          {isSidebarOpen && view !== "settings" && (
            <motion.div
              key="main-sidebar"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              transition={{ type: "spring", bounce: 0, duration: 0.3 }}
              className="flex-shrink-0 h-full flex"
            >
              <Sidebar
                sessions={sessions}
                projects={projects}
                activeProjectPath={activeProjectPath}
                activeId={activeId}
                onSelect={onSelect}
                onSelectProject={onSelectProject}
                onNewChat={onNewChat}
                onAddProject={onAddProject}
                onRemoveProject={onRemoveProject}
                onOpenSearch={() => setIsSearchOpen(true)}
                onOpenSkills={() => navigateTo(activeId, "skills")}
                onOpenAutomation={() => navigateTo(activeId, "automation")}
                onOpenSettings={() => navigateTo(activeId, "settings")}
                onLogout={onLogout}
              />
            </motion.div>
          )}
          {isSidebarOpen && view === "settings" && (
            <motion.div
              key="settings-sidebar"
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
              transition={{ type: "spring", bounce: 0, duration: 0.3 }}
              className="flex-shrink-0 h-full flex"
            >
              <SettingsSidebar 
                onBack={goBack} 
                activeCategoryId={activeSettingsCategory} 
                onSelectCategory={setActiveSettingsCategory} 
              />
            </motion.div>
          )}
        </AnimatePresence>

        <main className="flex-1 bg-white rounded-tl-2xl border-l border-t border-border-theme flex overflow-hidden shadow-sm relative">
          <AnimatePresence mode="wait">
            {view === "start" && (
              <motion.div 
                key="start" 
                initial={{ opacity: 0, y: 15 }} 
                animate={{ opacity: 1, y: 0 }} 
                exit={{ opacity: 0, y: -15 }} 
                transition={{ type: "spring", bounce: 0, duration: 0.3 }}
                className="w-full h-full flex flex-col"
              >
                <StartView 
                  projectName={activeProjectName} 
                  sessions={sessions}
                  activeId={activeId}
                  onSelectSession={onSelect}
                  suggestions={SUGGESTIONS} 
                  onSubmit={onSubmit} 
                />
              </motion.div>
            )}
            {view === "chat" && (
              <motion.div 
                key="chat" 
                initial={{ opacity: 0, y: 15 }} 
                animate={{ opacity: 1, y: 0 }} 
                exit={{ opacity: 0, y: -15 }} 
                transition={{ type: "spring", bounce: 0, duration: 0.3 }}
                className="w-full h-full flex flex-col"
              >
                <ChatView
                  messages={chatMessages}
                  onSend={onSubmit}
                  onFork={onForkSession}
                  onRewind={onRewindSession}
                  onExport={onExportSession}
                  timeline={detail?.timeline ?? []}
                />
              </motion.div>
            )}
            {view === "skills" && (
              <motion.div 
                key="skills" 
                initial={{ opacity: 0, y: 15 }} 
                animate={{ opacity: 1, y: 0 }} 
                exit={{ opacity: 0, y: -15 }} 
                transition={{ type: "spring", bounce: 0, duration: 0.3 }}
                className="w-full h-full flex flex-col"
              >
                <SkillsView />
              </motion.div>
            )}
            {view === "automation" && (
              <motion.div 
                key="automation" 
                initial={{ opacity: 0, y: 15 }} 
                animate={{ opacity: 1, y: 0 }} 
                exit={{ opacity: 0, y: -15 }} 
                transition={{ type: "spring", bounce: 0, duration: 0.3 }}
                className="w-full h-full flex flex-col"
              >
                <AutomationView />
              </motion.div>
            )}
            {view === "settings" && (
              <motion.div 
                key="settings" 
                initial={{ opacity: 0, y: 15 }} 
                animate={{ opacity: 1, y: 0 }} 
                exit={{ opacity: 0, y: -15 }} 
                transition={{ type: "spring", bounce: 0, duration: 0.3 }}
                className="w-full h-full flex flex-col"
              >
                <SettingsView activeCategoryId={activeSettingsCategory} />
              </motion.div>
            )}
          </AnimatePresence>
        </main>
      </div>

      {/* Overlays */}
      <SearchModal isOpen={isSearchOpen} onClose={() => setIsSearchOpen(false)} />
      <ApprovalDialog
        request={approvals[0] ?? null}
        queueCount={approvals.length}
        onApprove={(req) => onApprovalDecision(req, true)}
        onReject={(req) => onApprovalDecision(req, false)}
      />
    
      {showOnboarding && (
        <OnboardingWizard
          onComplete={() => {
            setShowOnboarding(false);
            refreshSessions();
          }}
        />
      )}
</div>
  );
}
