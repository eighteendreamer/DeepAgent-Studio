import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import {
  ARCHIVE_CHANGED_EVENT,
  OPEN_AUTOMATION_EVENT,
  archiveAllConversations,
  archiveConversation,
  getSessionDetail,
  getSessionConversation,
  listSessions,
  listProjects,
  getActiveProject,
  setActiveProject,
  addProject,
  archiveProjectConversations,
  openProjectInFileManager,
  pickProjectFolder,
  renameProject,
  removeProject,
  setProjectPinned,
  setSessionPinned,
  clearApiKey,
  getSettings,
  runChat,
  resolveApproval,
  stopChat,
  getPlanMode,
  forkSession,
  rewindSession,
  exportTranscript,
  saveTranscriptFile,
  openSessionInNewWindow,
  renameSession,
} from "./api";
import type { RuntimeEvent } from "./api";
import type {
  ApprovalRequest,
  ChatMessage,
  ComposerAttachment,
  ConversationMessage,
  MessagePart,
  PersistedAttachment,
  Project,
  SessionDetail,
  SessionSummary,
  ToolCall,
} from "./types";
import { TitleBar } from "./components/TitleBar";
import { Sidebar } from "./components/Sidebar";
import { StartView } from "./components/StartView";
import { ChatView } from "./components/ChatView";
import { SearchModal } from "./components/SearchModal";
import { SkillsView } from "./components/SkillsView";
import { KnowledgeView } from "./components/KnowledgeView";
import { PluginsView } from "./components/PluginsView";
import { AutomationView } from "./components/AutomationView";
import { OnboardingWizard } from "./components/OnboardingWizard";
import { SettingsSidebar } from "./components/SettingsSidebar";
import { SettingsView } from "./components/SettingsView";
import { message } from "./components/message";

type View = "start" | "chat" | "skills" | "knowledge" | "plugins" | "automation" | "settings";

const LEFT_SIDEBAR_OPEN_KEY = "deepagent:left-sidebar-open";

function readLeftSidebarOpen(): boolean {
  if (typeof window === "undefined") return true;
  return window.localStorage.getItem(LEFT_SIDEBAR_OPEN_KEY) !== "false";
}

interface SessionCompletedPayload {
  run_id: string;
  session_id: string | null;
  status: "completed" | "failed" | string;
  error?: string | null;
}

interface SessionTitleUpdatedPayload {
  session_id: string;
  title: string;
}

function mapPersistedAttachment(attachment: PersistedAttachment): ComposerAttachment {
  return {
    id: attachment.id,
    kind: attachment.kind,
    name: attachment.name,
    mime: attachment.mime,
    size: attachment.size_bytes,
    source: attachment.source,
    localPath: attachment.original_path ?? undefined,
    originalPath: attachment.original_path ?? undefined,
    extractedText: attachment.extracted_text ?? undefined,
    storageDir: attachment.storage_dir,
    sha256: attachment.sha256 ?? undefined,
    backendMessage: attachment.message ?? undefined,
    status: attachment.status,
    error: attachment.status === "error" ? attachment.message ?? undefined : undefined,
  };
}

function mapConversationToChatMessages(conversation: ConversationMessage[]): ChatMessage[] {
  return conversation.map((m) => ({
    role: m.role,
    content: m.content,
    attachments: m.attachments?.map(mapPersistedAttachment),
    usage: m.usage
      ? {
          promptTokens: m.usage.prompt_tokens,
          completionTokens: m.usage.completion_tokens,
          totalTokens: m.usage.total_tokens,
          cacheHitTokens: m.usage.prompt_cache_hit_tokens,
          cacheMissTokens: m.usage.prompt_cache_miss_tokens,
          costYuan: m.usage.cost_yuan,
        }
      : undefined,
    runMs: m.usage?.duration_ms,
    parts: m.parts.map((p): MessagePart => {
      if (p.kind === "tool") {
        return {
          kind: "tool",
          tool: {
            call_id: p.call_id,
            name: p.name,
            args: p.args,
            status:
              p.status === "ok"
                ? "ok"
                : p.status === "error"
                ? "error"
                : p.status === "blocked"
                ? "blocked"
                : "running",
            durationMs: p.duration_ms ?? undefined,
            detail: p.detail ?? undefined,
            output: p.output ?? undefined,
          },
        };
      }
      if (p.kind === "reasoning") return { kind: "reasoning", text: p.text };
      return { kind: "text", text: p.text };
    }),
  }));
}

function buildPromptWithAttachments(text: string, attachments: ComposerAttachment[] = []): string {
  const trimmed = text.trim();
  if (attachments.length === 0) return trimmed;
  const blocks = attachments.map((attachment, index) => {
    const lines = [
      `<attachment index="${index + 1}" name="${attachment.name}" type="${attachment.mime}" kind="${attachment.kind}">`,
      `source: ${attachment.source}`,
      `size: ${attachment.size} bytes`,
    ];
    if (attachment.originalPath) {
      lines.push(`path: ${attachment.originalPath}`);
    }
    if (attachment.sha256) {
      lines.push(`sha256: ${attachment.sha256}`);
    }
    if (attachment.extractedText) {
      lines.push("", attachment.extractedText);
    } else if (attachment.kind === "image") {
      lines.push(
        "",
        "This image is attached but could not be analyzed by the system vision runtime. Do not infer details that are not present. If the image content is essential, ask the user to install the vision model or describe the image.",
      );
    } else {
      lines.push("", "Binary or unsupported file content was not read. Use file tools if deeper inspection is needed.");
    }
    lines.push("</attachment>");
    return lines.join("\n");
  });
  return [trimmed || "\u8bf7\u67e5\u770b\u9644\u4ef6\u5185\u5bb9\u3002", "", "<attachments>", blocks.join("\n\n"), "</attachments>"].join("\n");
}

export function App() {
  const requestedSessionIdRef = useRef<string | null>(null);
  if (requestedSessionIdRef.current === null && typeof window !== "undefined") {
    const raw = window.location.hash.startsWith("#")
      ? window.location.hash.slice(1)
      : window.location.hash;
    const params = new URLSearchParams(raw);
    requestedSessionIdRef.current = params.get("session");
  }

  const [showOnboarding, setShowOnboarding] = useState(
    () => !localStorage.getItem("onboarding_complete")
  );
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [activeProjectPath, setActiveProjectPath] = useState<string | null>(null);
  const [projectMapOpenSignal, setProjectMapOpenSignal] = useState(0);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [view, setView] = useState<View>("start");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  // FIFO queue of tool-approval requests awaiting the user's decision.
  const [approvals, setApprovals] = useState<ApprovalRequest[]>([]);
  // True while an agent run is streaming. Used to gate new submissions (you
  // can't send a new message mid-output) and to show a busy send button. The
  // run itself is async on the backend, so the UI never freezes — approvals,
  // scrolling and navigation stay responsive while this is true.
  const [runningSessionIds, setRunningSessionIds] = useState<Set<string>>(() => new Set());
  const [planMode, setPlanMode] = useState(false);
  // The session id of the in-flight run, set as soon as the backend announces
  // it (session_registered) — used for the manual stop button and to navigate
  // into the still-running session.
  const [activePendingRunKey, setActivePendingRunKey] = useState<string | null>(null);

  // The in-flight transcript, kept per-session in a ref so it SURVIVES
  // navigation. Leaving and returning to a running session restores its live
  // messages from here (not a lossy DB/timeline reload). Keyed by session id;
  // a not-yet-registered new run uses the "__pending__" key until its id is
  // known, then it is migrated.
  const liveTranscripts = useRef<Map<string, ChatMessage[]>>(new Map());
  // Always-current activeId, readable from inside the long-lived run handler.
  const activeIdRef = useRef<string | null>(null);
  const activePendingRunKeyRef = useRef<string | null>(null);
  // Always-current `messages`, so a continuation can read the on-screen thread
  // without taking a stale closure (and without re-creating onSubmit).
  const messagesRef = useRef<ChatMessage[]>([]);

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
          const requested = requestedSessionIdRef.current;
          const requestedMatch = s.find((session) => session.id === requested) ?? null;
          const initialId = requestedMatch?.id ?? s[0].id;
          setActiveId(initialId);
          setNavState({
            history: [{ activeId: initialId, view: requestedMatch ? "chat" : "start" }],
            index: 0,
          });
          if (requestedMatch) {
            setView("chat");
          }
        }
      })
      .catch(() => setSessions([]));
  }, []);

  // Reconcile the onboarding gate with the backend's real state: the source of
  // truth for "logged in" is whether the keychain actually holds a valid key
  // (settings.configured), not the local flag. This self-heals cases where the
  // localStorage flag is stale (e.g. set by an older build or after the key was
  // cleared) so the user is never dropped into the app without a usable key.
  useEffect(() => {
    getSettings()
      .then((view) => {
        const configured = !!view?.configured;
        setShowOnboarding(!configured);
        if (configured) {
          localStorage.setItem("onboarding_complete", "true");
        } else {
          localStorage.removeItem("onboarding_complete");
        }
      })
      .catch(() => {
        // Backend unreachable (e.g. browser preview): keep the local flag.
      });
  }, []);

  useEffect(() => {
    activeIdRef.current = activeId;
    if (!activeId) {
      setDetail(null);
      setPlanMode(false);
      return;
    }
    getSessionDetail(activeId)
      .then(setDetail)
      .catch(() => setDetail(null));
    getPlanMode(activeId)
      .then(setPlanMode)
      .catch(() => setPlanMode(false));
  }, [activeId]);

  // Keep a ref mirror of `messages` for stale-closure-free reads in onSubmit.
  useEffect(() => {
    messagesRef.current = messages;
  }, [messages]);

  useEffect(() => {
    activePendingRunKeyRef.current = activePendingRunKey;
  }, [activePendingRunKey]);

  // Populate the chat view when the active session changes. Two sources:
  // 1. If this session has a live (in-flight or just-finished) transcript in
  //    the ref, restore it verbatim — leaving and returning to a RUNNING
  //    session shows exactly what was streaming, no lossy reload.
  // 2. Otherwise load the reconstructed, styled conversation from the backend
  //    (tool cards / reasoning / text), so a past session looks as it did live.
  useEffect(() => {
    if (view !== "chat" || !activeId) return;

    const live = liveTranscripts.current.get(activeId);
    if (live) {
      setMessages(live);
      return;
    }

    let cancelled = false;
    getSessionConversation(activeId)
      .then((conv) => {
        if (cancelled) return;
        const mapped = mapConversationToChatMessages(conv);
        // Don't clobber a live transcript that appeared while loading.
        if (!liveTranscripts.current.get(activeId)) {
          setMessages(mapped);
        }
      })
      .catch(() => {
        /* leave the view empty rather than showing raw timeline lines */
      });
    return () => {
      cancelled = true;
    };
  }, [view, activeId]);

  // The active project's display name (folder name), for the StartView header.
  const activeProjectName = useMemo(() => {
    const p = projects.find((p) => p.path === activeProjectPath);
    return p?.name ?? projects[0]?.name ?? "";
  }, [projects, activeProjectPath]);
  const activeSession = useMemo(
    () => sessions.find((s) => s.id === activeId) ?? null,
    [activeId, sessions]
  );

  const navigateTo = useCallback((newActiveId: string | null, newView: View) => {
    if (newActiveId === activeId && newView === view) {
      return;
    }

    setActiveId(newActiveId);
    setView(newView);
    setMessages([]);
    setActivePendingRunKey(null);
    setNavState((prev) => {
      const newHistory = prev.history.slice(0, prev.index + 1);
      newHistory.push({ activeId: newActiveId, view: newView });
      return { history: newHistory, index: newHistory.length - 1 };
    });
  }, [activeId, view]);

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
    return Promise.all([
      listSessions().then(setSessions),
      listProjects().then(setProjects),
    ]).catch(() => {});
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/event")
      .then((mod) =>
        mod.listen<SessionCompletedPayload>("session://completed", (event) => {
          const payload = event.payload;
          if (payload.session_id) {
            setRunningSessionIds((prev) => {
              const next = new Set(prev);
              next.delete(payload.session_id as string);
              return next;
            });
          }
          refreshSessions();
          if (payload.status === "failed") {
            message.error(`会话运行失败：${payload.error ?? "unknown error"}`);
          } else {
            message.success("会话已完成");
          }
        })
      )
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshSessions]);

  useEffect(() => {
    window.addEventListener(ARCHIVE_CHANGED_EVENT, refreshSessions);
    return () => window.removeEventListener(ARCHIVE_CHANGED_EVENT, refreshSessions);
  }, [refreshSessions]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/event")
      .then((mod) =>
        mod.listen<SessionTitleUpdatedPayload>("session://title-updated", (event) => {
          const payload = event.payload;
          setSessions((prev) =>
            prev.map((session) =>
              session.id === payload.session_id ? { ...session, title: payload.title } : session
            )
          );
          setDetail((prev) =>
            prev && prev.summary.id === payload.session_id
              ? { ...prev, summary: { ...prev.summary, title: payload.title } }
              : prev
          );
          refreshSessions();
        })
      )
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshSessions]);

  useEffect(() => {
    const onOpenAutomation = () => navigateTo(activeIdRef.current, "automation");
    window.addEventListener(OPEN_AUTOMATION_EVENT, onOpenAutomation);
    return () => window.removeEventListener(OPEN_AUTOMATION_EVENT, onOpenAutomation);
  }, [navigateTo]);

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
      .then(() => Promise.all([listProjects(), getActiveProject(), listSessions()]))
      .then(([ps, active, nextSessions]) => {
        setProjects(ps);
        setActiveProjectPath(active);
        setSessions(nextSessions);
        if (activeId && !nextSessions.some((s) => s.id === activeId)) {
          setActiveId(null);
          setDetail(null);
          setMessages([]);
          setView("start");
        }
        message.success("已移除项目");
      })
      .catch((err) => {
        message.error(`移除失败：${String(err)}`);
      });
  }, [activeId]);

  const onPinSession = useCallback(
    (sessionId: string, pinned: boolean) => {
      setSessionPinned(sessionId, pinned)
        .then(() => refreshSessions())
        .then(() => {
          message.success(pinned ? "已置顶会话" : "已取消置顶会话");
        })
        .catch((err) => {
          message.error(`更新置顶失败：${String(err)}`);
        });
    },
    [refreshSessions]
  );

  const onArchiveSession = useCallback(
    (sessionId: string) => {
      archiveConversation(sessionId)
        .then((archived) => {
          refreshSessions();
          if (activeId === sessionId) {
            setActiveId(null);
            setDetail(null);
            setMessages([]);
            setView("start");
          }
          message.success(archived ? "已归档对话" : "对话已在归档中");
        })
        .catch((err) => {
          message.error(`归档失败：${String(err)}`);
        });
    },
    [activeId, refreshSessions]
  );

  const onArchiveAllSessions = useCallback(() => {
    archiveAllConversations()
      .then((archivedCount) => {
        refreshSessions();
        if (activeId) {
          setActiveId(null);
          setDetail(null);
          setMessages([]);
          setView("start");
        }
        message.success(
          archivedCount > 0 ? `已归档 ${archivedCount} 个对话` : "没有可归档的对话"
        );
      })
      .catch((err) => {
        message.error(`归档失败：${String(err)}`);
      });
  }, [activeId, refreshSessions]);

  const onPinProject = useCallback(
    (path: string, pinned: boolean) => {
      setProjectPinned(path, pinned)
        .then(() => refreshSessions())
        .then(() => {
          message.success(pinned ? "已置顶项目" : "已取消置顶项目");
        })
        .catch((err) => {
          message.error(`更新置顶失败：${String(err)}`);
        });
    },
    [refreshSessions]
  );

  const onOpenProject = useCallback((path: string) => {
    openProjectInFileManager(path).catch((err) => {
      message.error(`打开失败：${String(err)}`);
    });
  }, []);

  const onOpenProjectMap = useCallback((path: string) => {
    setActiveProjectPath(path);
    setActiveProject(path).catch(() => {});
    setProjectMapOpenSignal((n) => n + 1);
    if (view !== "chat" && view !== "start") {
      setView("start");
    }
  }, [view]);

  const onRenameProject = useCallback(
    (path: string, name: string) => {
      renameProject(path, name)
        .then(() => refreshSessions())
        .then(() => {
          message.success("已重命名项目");
        })
        .catch((err) => {
          message.error(`重命名失败：${String(err)}`);
        });
    },
    [refreshSessions]
  );

  const onArchiveProject = useCallback(
    (path: string, name: string) => {
      archiveProjectConversations(path)
        .then((result) => {
          refreshSessions();
          const activeSession = sessions.find((s) => s.id === activeId);
          if (activeSession?.project === name) {
            setActiveId(null);
            setDetail(null);
            setMessages([]);
            setView("start");
          }
          message.success(
            result.archived_count > 0
              ? `已归档 ${result.archived_count} 个对话`
              : "没有可归档的对话"
          );
        })
        .catch((err) => {
          message.error(`归档失败：${String(err)}`);
        });
    },
    [activeId, refreshSessions, sessions]
  );

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
    (
      text: string,
      attachments: ComposerAttachment[] = [],
      envMode?: "local" | "remote",
      connectionId?: string | null
    ) => {
      const submittedText = buildPromptWithAttachments(text, attachments);
      if (!submittedText) return;
      const visibleUserText = text.trim() || (attachments.length > 0 ? "请查看附件内容。" : "");
      const storedEnvMode = localStorage.getItem("envMode");
      const effectiveEnvMode: "local" | "remote" =
        envMode ?? (storedEnvMode === "remote" ? "remote" : "local");
      const storedConnectionId = localStorage.getItem("ssh_connection_id");
      const effectiveConnectionId =
        effectiveEnvMode === "remote"
          ? connectionId === undefined
            ? storedConnectionId
            : connectionId
          : null;
      // Block new submissions while a run is streaming — you can't start a new
      // message until the current output finishes.
      // Continue the current session when submitting a follow-up from within an
      // open chat; start a fresh session when submitting from the start view.
      const continueId = view === "chat" && activeId ? activeId : null;
      if (continueId && runningSessionIds.has(continueId)) return;
      if (!continueId && activePendingRunKey) return;

      // Wall-clock start of this run, for the "total time" footer metric.
      const runStart = Date.now();

      // The run's session key for the live transcript. Until the backend
      // announces the real id (session_registered), a new run buffers under a
      // pending key; continuations use the existing id directly.
      const runId =
        globalThis.crypto?.randomUUID?.() ??
        `run_${Date.now()}_${Math.random().toString(16).slice(2)}`;
      const pendingKey = `__pending__:${runId}`;
      let runKey = continueId ?? pendingKey;
      if (continueId) {
        setRunningSessionIds((prev) => {
          const next = new Set(prev);
          next.add(continueId);
          return next;
        });
      } else {
        setActivePendingRunKey(pendingKey);
      }

      // Seed the transcript: continuations append to the existing one; a fresh
      // run starts clean. Stored in the ref so it survives navigation.
      const prior: ChatMessage[] = continueId
        ? liveTranscripts.current.get(continueId) ?? messagesRef.current
        : [];
      const seeded: ChatMessage[] = [
        ...prior,
        { role: "user", content: visibleUserText, attachments },
        { role: "assistant", content: "", parts: [] },
      ];
      liveTranscripts.current.set(runKey, seeded);
      setMessages(seeded);

      if (view !== "chat") {
        setView("chat");
        setNavState((prev) => {
          const newHistory = prev.history.slice(0, prev.index + 1);
          newHistory.push({ activeId, view: "chat" });
          return { history: newHistory, index: newHistory.length - 1 };
        });
      }

      // Apply an update to the run's transcript (in the ref) and, only if that
      // session is the one currently on screen, mirror it to `messages`. This
      // is what lets a run keep streaming into its buffer while the user is on
      // a different page — and show up intact when they return.
      const updateTranscript = (fn: (prev: ChatMessage[]) => ChatMessage[]) => {
        const current = liveTranscripts.current.get(runKey) ?? [];
        const updated = fn(current);
        liveTranscripts.current.set(runKey, updated);
        if (activeIdRef.current === runKey || activePendingRunKeyRef.current === runKey) {
          setMessages(updated);
        }
      };

      // --- ordered-parts helpers ------------------------------------------
      // An assistant turn is an ordered list of parts (reasoning / tool / text)
      // appended in the EXACT order events stream in, so the UI preserves
      // chronology (reasoning before a tool that came after it, etc.) instead
      // of grouping all tools above all text.
      const mutateParts = (fn: (parts: MessagePart[]) => MessagePart[]) => {
        updateTranscript((prev) => {
          const next = [...prev];
          const lastIdx = next.length - 1;
          if (lastIdx < 0 || next[lastIdx].role !== "assistant") return prev;
          const msg = next[lastIdx];
          const parts = fn(msg.parts ? [...msg.parts] : []);
          next[lastIdx] = { ...msg, parts };
          return next;
        });
      };

      // Append a streaming text/reasoning delta. Extends the last part when it
      // is the same kind (a continuous stream), else opens a new part — so a
      // tool call between two text bursts splits them into separate segments.
      const appendDelta = (kind: "reasoning" | "text", delta: string) => {
        if (!delta) return;
        mutateParts((parts) => {
          const last = parts[parts.length - 1];
          if (last && last.kind === kind) {
            parts[parts.length - 1] = { ...last, text: last.text + delta } as MessagePart;
          } else {
            parts.push(
              kind === "reasoning"
                ? { kind: "reasoning", text: delta }
                : { kind: "text", text: delta }
            );
          }
          return parts;
        });
      };

      // Insert or update a tool part keyed by call_id, preserving its position.
      const upsertTool = (callId: string, patch: Partial<ToolCall> & { name?: string }) => {
        mutateParts((parts) => {
          const idx = parts.findIndex((p) => p.kind === "tool" && p.tool.call_id === callId);
          if (idx >= 0) {
            const prevTool = (parts[idx] as { kind: "tool"; tool: ToolCall }).tool;
            parts[idx] = { kind: "tool", tool: { ...prevTool, ...patch } };
          } else {
            parts.push({
              kind: "tool",
              tool: {
                call_id: callId,
                name: patch.name ?? "tool",
                args: patch.args ?? "",
                status: patch.status ?? "running",
                durationMs: patch.durationMs,
                detail: patch.detail,
                output: patch.output,
              },
            });
          }
          return parts;
        });
      };

      // Accumulate token usage onto the (last) assistant message of the run.
      const addUsage = (u: {
        prompt: number;
        completion: number;
        total: number;
        cacheHit: number;
        cacheMiss: number;
        costYuan?: number;
      }) => {
        updateTranscript((prev) => {
          const next = [...prev];
          const lastIdx = next.length - 1;
          if (lastIdx < 0 || next[lastIdx].role !== "assistant") return prev;
          const msg = next[lastIdx];
          const cur = msg.usage ?? {
            promptTokens: 0,
            completionTokens: 0,
            totalTokens: 0,
            cacheHitTokens: 0,
            cacheMissTokens: 0,
          };
          next[lastIdx] = {
            ...msg,
            usage: {
              promptTokens: cur.promptTokens + u.prompt,
              completionTokens: cur.completionTokens + u.completion,
              totalTokens: cur.totalTokens + u.total,
              cacheHitTokens: cur.cacheHitTokens + u.cacheHit,
              cacheMissTokens: cur.cacheMissTokens + u.cacheMiss,
              costYuan: u.costYuan ?? cur.costYuan,
            },
          };
          return next;
        });
      };

      // Finalize the assistant turn once the run ends. Marks the tone on text
      // parts (for errors) and ensures the authoritative final message is
      // present if the stream produced no visible text (e.g. tool-only turn).
      const finalize = (finalContent: string, tone?: "normal" | "error") => {
        updateTranscript((prev) => {
          const next = [...prev];
          const lastIdx = next.length - 1;
          if (lastIdx < 0 || next[lastIdx].role !== "assistant") return prev;
          const msg = next[lastIdx];
          const parts = msg.parts ? [...msg.parts] : [];
          const fin = finalContent.trim();
          const streamedText = parts
            .filter((p) => p.kind === "text")
            .map((p) => (p as { text: string }).text)
            .join("")
            .trim();

          if (tone === "error") {
            // Append the error as its own trailing text part.
            parts.push({ kind: "text", text: fin, tone: "error" });
          } else if (streamedText.length === 0 && fin.length > 0) {
            // No visible text streamed but the run has a final message: show it.
            parts.push({ kind: "text", text: fin });
          } else if (streamedText.length === 0 && fin.length === 0) {
            // The provider completed without any visible content. Keep the turn
            // explicit instead of leaving a blank assistant block on screen.
            parts.push({
              kind: "text",
              text: "模型未返回可见内容，请重试或切换模型。",
              tone: "error",
            });
          }

          // Keep a flat `content` mirror for transcript/export and copy.
          const content =
            parts
              .filter((p) => p.kind === "text")
              .map((p) => (p as { text: string }).text)
              .join("\n")
              .trim() || fin;

          next[lastIdx] = {
            ...msg,
            parts,
            content,
            runMs: Date.now() - runStart,
            ...(tone ? { tone } : {}),
          };
          return next;
        });
      };

      // Summarize a tool's JSON output into a short one-line detail string.
      const summarizeWebSearch = (output: Record<string, unknown>): string => {
        if (typeof output.error === "string") return output.error.slice(0, 200);
        const provider = typeof output.provider === "string" ? output.provider : "unknown";
        const count =
          typeof output.count === "number"
            ? output.count
            : Array.isArray(output.results)
            ? output.results.length
            : 0;
        const attempts = Array.isArray(output.attempts)
          ? output.attempts
              .map((item) => {
                if (!item || typeof item !== "object") return null;
                const attempt = item as Record<string, unknown>;
                const name = typeof attempt.provider === "string" ? attempt.provider : null;
                if (!name) return null;
                return `${name}:${attempt.ok === true ? "ok" : "failed"}`;
              })
              .filter(Boolean)
              .join(", ")
          : "";
        const suffix = attempts ? `; attempts: ${attempts}` : "";
        return `web_search: ${provider} returned ${count} result(s)${suffix}`;
      };

      const summarize = (toolName: string, output: unknown): string => {
        if (output == null) return "";
        if (typeof output === "string") return output.slice(0, 200);
        try {
          const o = output as Record<string, unknown>;
          if (toolName === "web_search") return summarizeWebSearch(o);
          if (typeof o.error === "string") return o.error.slice(0, 200);
          if (Array.isArray(o.results)) return `${o.results.length} result(s)`;
          if (typeof o.count === "number") return `${o.count} result(s)`;
          if (typeof o.path === "string") return String(o.path);
          const s = JSON.stringify(output);
          return s.length > 200 ? s.slice(0, 200) + "…" : s;
        } catch {
          return "";
        }
      };

      const onEvent = (event: RuntimeEvent) => {
        switch (event.type) {
          case "session_registered": {
            // The backing session now exists. Migrate the pending transcript to
            // its real id, register it for navigation, and point future updates
            // at the real key — so the user can leave & return to the in-flight
            // chat and still see its live stream.
            const sid = String(event.session_id ?? "");
            if (sid) {
              if (runKey !== sid) {
                const buf = liveTranscripts.current.get(runKey);
                if (buf) {
                  liveTranscripts.current.delete(runKey);
                  liveTranscripts.current.set(sid, buf);
                }
                runKey = sid;
              }
              setActivePendingRunKey((current) => (current === pendingKey ? null : current));
              setRunningSessionIds((prev) => {
                const next = new Set(prev);
                next.add(sid);
                return next;
              });
              setActiveId(sid);
              setNavState((prev) => {
                const last = prev.history[prev.index];
                if (last && last.activeId === sid && last.view === "chat") return prev;
                const newHistory = prev.history.slice(0, prev.index + 1);
                newHistory.push({ activeId: sid, view: "chat" });
                return { history: newHistory, index: newHistory.length - 1 };
              });
              refreshSessions();
            }
            break;
          }
          case "reasoning_delta":
            // Live Thinking-Mode trace.
            appendDelta("reasoning", String(event.text ?? ""));
            break;
          case "content_delta":
            // Live visible content (token-by-token).
            appendDelta("text", String(event.text ?? ""));
            break;
          case "tool_started":
            upsertTool(String(event.call_id ?? ""), {
              name: String(event.name ?? "tool"),
              args: event.arguments ? JSON.stringify(event.arguments, null, 2) : "",
              status: "running",
            });
            break;
          case "tool_completed":
            upsertTool(String(event.call_id ?? ""), {
              name: String(event.name ?? "tool"),
              status: event.ok ? "ok" : "error",
              durationMs:
                typeof event.duration_ms === "number" ? event.duration_ms : undefined,
              detail: summarize(String(event.name ?? "tool"), event.output),
              output: event.output,
            });
            break;
          case "tool_blocked":
            // Approval requests are handled via the dialog; only hard denials
            // mark the card blocked here.
            if (!event.needs_approval) {
              // Match by name when no call_id is provided on the event.
              upsertTool(String(event.name ?? "tool"), {
                name: String(event.name ?? "tool"),
                status: "blocked",
                detail: String(event.reason ?? ""),
              });
            }
            break;
          case "usage":
            addUsage({
              prompt: Number(event.prompt_tokens ?? 0),
              completion: Number(event.completion_tokens ?? 0),
              total: Number(event.total_tokens ?? 0),
              cacheHit: Number(event.prompt_cache_hit_tokens ?? 0),
              cacheMiss: Number(event.prompt_cache_miss_tokens ?? 0),
              costYuan: typeof event.cost_yuan === "number" ? event.cost_yuan : undefined,
            });
            break;
          case "run_completed":
            // Reconcile streamed content with the authoritative final message.
            finalize(String(event.message ?? ""), undefined);
            break;
          case "run_failed":
            finalize(`run failed: ${String(event.reason ?? "unknown error")}`, "error");
            break;
          case "run_cancelled":
            finalize("（已手动停止）", "error");
            break;
          default:
            break;
        }
      };

      runChat(
        submittedText,
        onEvent,
        (request) => {
          // Queue the approval request; the dialog shows the head of the queue.
          setApprovals((prev) => [...prev, request]);
        },
        continueId,
        runId,
        effectiveEnvMode,
        effectiveConnectionId
      )
        .then((newSessionId) => {
          // The run created (or continued) a session under the active project;
          // refresh the sidebar lists and focus that session.
          refreshSessions();
          if (newSessionId) {
            getPlanMode(newSessionId)
              .then(setPlanMode)
              .catch(() => {});
          }
          if (newSessionId && newSessionId !== activeIdRef.current) {
            setActiveId(newSessionId);
            setNavState((prev) => {
              const newHistory = prev.history.slice(0, prev.index + 1);
              newHistory.push({ activeId: newSessionId, view: "chat" });
              return { history: newHistory, index: newHistory.length - 1 };
            });
          }
        })
        .catch((err) => {
          updateTranscript((prev) => {
            const next = [...prev];
            const lastIdx = next.length - 1;
            if (lastIdx >= 0 && next[lastIdx].role === "assistant") {
              next[lastIdx] = {
                ...next[lastIdx],
                content: `error: ${String(err)}`,
                tone: "error",
              };
            }
            return next;
          });
        })
        .finally(() => {
          setRunningSessionIds((prev) => {
            const next = new Set(prev);
            next.delete(runKey);
            if (continueId) next.delete(continueId);
            return next;
          });
          setActivePendingRunKey((current) => (current === pendingKey ? null : current));
          setApprovals((prev) => prev.filter((a) => a.run_id !== runId));
          // Keep the finished session's live transcript in memory so returning
          // to it preserves streamed reasoning deltas. The DB replay currently
          // reconstructs tool cards and final messages, but tool-call reasoning
          // only exists in the live stream. Only clear the temporary pending key.
          if (pendingKey !== runKey) {
            liveTranscripts.current.delete(pendingKey);
          }
        });
    },
    [activeId, view, refreshSessions, runningSessionIds, activePendingRunKey]
  );

  const chatMessages = messages;

  // Manually stop the in-flight run (the run ends cleanly at the next step).
  const onStopRun = useCallback(() => {
    const sid = activeId && runningSessionIds.has(activeId) ? activeId : null;
    if (sid) stopChat(sid).catch(() => {});
  }, [runningSessionIds, activeId]);

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
    async (toSeq: number) => {
      if (!activeId) return;
      try {
        liveTranscripts.current.delete(activeId);
        setMessages([]);
        await rewindSession(activeId, toSeq);
        const [nextDetail, nextConversation] = await Promise.all([
          getSessionDetail(activeId),
          getSessionConversation(activeId),
          refreshSessions(),
        ]);
        setDetail(nextDetail);
        setMessages(mapConversationToChatMessages(nextConversation));
      } catch {
        // keep current screen state if rewind fails
      }
    },
    [activeId, refreshSessions]
  );

  // Export the active session transcript and trigger a browser/Tauri download.
  const onExportSession = useCallback(
    async (format: "markdown" | "json") => {
      if (!activeId) return;
      try {
        const transcript = await exportTranscript(activeId, format);
        const rawTitle = activeSession?.title ?? detail?.summary.title ?? "";
        const safeTitle = rawTitle
          .trim()
          .replace(/[<>:"/\\|?*\u0000-\u001F]/g, "_")
          .slice(0, 80);
        const suggestedName = `${safeTitle || `session-${activeId}`}.${transcript.extension}`;
        const savedPath = await saveTranscriptFile(transcript, suggestedName);
        if (!savedPath) return;
        message.success(format === "json" ? "JSON 已导出" : "对话已导出");
      } catch (err) {
        message.error(`导出失败：${String(err)}`);
      }
    },
    [activeId, activeSession?.title, detail?.summary.title]
  );

  const onCopySession = useCallback(() => {
    if (!activeId) return;
    exportTranscript(activeId, "markdown")
      .then(async (transcript) => {
        if (navigator.clipboard?.writeText) {
          await navigator.clipboard.writeText(transcript.content);
        } else {
          const textarea = document.createElement("textarea");
          textarea.value = transcript.content;
          textarea.style.position = "fixed";
          textarea.style.opacity = "0";
          document.body.appendChild(textarea);
          textarea.select();
          document.execCommand("copy");
          document.body.removeChild(textarea);
        }
        message.success("已复制对话");
      })
      .catch((err) => {
        message.error(`复制失败：${String(err)}`);
      });
  }, [activeId]);

  const onRenameSession = useCallback(
    async (title: string) => {
      if (!activeId) return;
      try {
        const summary = await renameSession(activeId, title);
        setSessions((prev) =>
          prev.map((session) => (session.id === summary.id ? summary : session))
        );
        setDetail((prev) => (prev ? { ...prev, summary } : prev));
        await refreshSessions();
        const latest = await getSessionDetail(activeId);
        setDetail(latest);
        message.success("已重命名对话");
      } catch (err) {
        message.error(`重命名失败：${String(err)}`);
      }
    },
    [activeId, refreshSessions]
  );

  const onOpenSessionInNewWindow = useCallback(() => {
    if (!activeId) return;
    openSessionInNewWindow(activeId)
      .catch((err) => {
        message.error(`打开新窗口失败：${String(err)}`);
      });
  }, [activeId]);

  const [isSidebarOpen, setIsSidebarOpen] = useState(readLeftSidebarOpen);
  const [isSearchOpen, setIsSearchOpen] = useState(false);

  // Settings State
  const [activeSettingsCategory, setActiveSettingsCategory] = useState("general");

  useEffect(() => {
    window.localStorage.setItem(LEFT_SIDEBAR_OPEN_KEY, String(isSidebarOpen));
  }, [isSidebarOpen]);

  const canGoBack = navState.index > 0;
  const canGoForward = navState.index < navState.history.length - 1;
  const activeChatBusy = Boolean(
    activePendingRunKey || (activeId && runningSessionIds.has(activeId))
  );

  return (
    <div className="bg-sidebar-bg text-text-base font-sans h-screen w-full overflow-hidden flex flex-col relative">
      <TitleBar 
        onToggleSidebar={() => setIsSidebarOpen((open) => !open)}
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
              initial={{ width: 0, opacity: 0, x: -20 }}
              animate={{ width: 240, opacity: 1, x: 0 }}
              exit={{ width: 0, opacity: 0, x: -20 }}
              transition={{ type: "spring", bounce: 0, duration: 0.3 }}
              className="flex-shrink-0 h-full flex overflow-hidden"
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
                onPinSession={onPinSession}
                onArchiveSession={onArchiveSession}
                onArchiveAllSessions={onArchiveAllSessions}
                onRemoveProject={onRemoveProject}
                onPinProject={onPinProject}
                onOpenProject={onOpenProject}
                onOpenProjectMap={onOpenProjectMap}
                onRenameProject={onRenameProject}
                onArchiveProject={onArchiveProject}
                onOpenSearch={() => setIsSearchOpen(true)}
                activeSurface={
                  view === "skills" || view === "knowledge" || view === "plugins" || view === "automation"
                    ? view
                    : null
                }
                onOpenSkills={() => navigateTo(activeId, "skills")}
                onOpenKnowledge={() => navigateTo(activeId, "knowledge")}
                onOpenPlugins={() => navigateTo(activeId, "plugins")}
                onOpenAutomation={() => navigateTo(activeId, "automation")}
                onOpenSettings={() => navigateTo(activeId, "settings")}
                onLogout={onLogout}
                runningSessionIds={runningSessionIds}
              />
            </motion.div>
          )}
          {isSidebarOpen && view === "settings" && (
            <motion.div
              key="settings-sidebar"
              initial={{ width: 0, opacity: 0, x: -20 }}
              animate={{ width: 240, opacity: 1, x: 0 }}
              exit={{ width: 0, opacity: 0, x: -20 }}
              transition={{ type: "spring", bounce: 0, duration: 0.3 }}
              className="flex-shrink-0 h-full flex overflow-hidden"
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
                  activeProjectPath={activeProjectPath}
                  projectMapOpenSignal={projectMapOpenSignal}
                  projects={projects}
                  onSelectProject={onSelectProject}
                  onAddProject={onAddProject}
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
                  sessionId={activeId}
                  sessionKey={activeId ?? activePendingRunKey ?? null}
                  messages={chatMessages}
                  onSend={onSubmit}
                  onFork={onForkSession}
                  onRewind={onRewindSession}
                  onExport={onExportSession}
                  onCopy={onCopySession}
                  onRename={onRenameSession}
                  onOpenInNewWindow={onOpenSessionInNewWindow}
                  onPin={() => {
                    if (activeSession) onPinSession(activeSession.id, !activeSession.pinned);
                  }}
                  onArchive={() => {
                    if (activeSession) onArchiveSession(activeSession.id);
                  }}
                  pinned={activeSession?.pinned ?? false}
                  title={activeSession?.title ?? detail?.summary.title ?? null}
                  timeline={detail?.timeline ?? []}
                  approval={approvals[0] ?? null}
                  approvalQueueCount={approvals.length}
                  onApprovalDecision={onApprovalDecision}
                  busy={activeChatBusy}
                  onStop={onStopRun}
                  planMode={planMode}
                  activeProjectPath={activeProjectPath}
                  projectMapOpenSignal={projectMapOpenSignal}
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
            {view === "knowledge" && (
              <motion.div 
                key="knowledge" 
                initial={{ opacity: 0, y: 15 }} 
                animate={{ opacity: 1, y: 0 }} 
                exit={{ opacity: 0, y: -15 }} 
                transition={{ type: "spring", bounce: 0, duration: 0.3 }}
                className="w-full h-full flex flex-col"
              >
                <KnowledgeView />
              </motion.div>
            )}
            {view === "plugins" && (
              <motion.div
                key="plugins"
                initial={{ opacity: 0, y: 15 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -15 }}
                transition={{ type: "spring", bounce: 0, duration: 0.3 }}
                className="w-full h-full flex flex-col"
              >
                <PluginsView />
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
      <SearchModal
        isOpen={isSearchOpen}
        onClose={() => setIsSearchOpen(false)}
        sessions={sessions}
        projects={projects}
        onSelectSession={onSelect}
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
