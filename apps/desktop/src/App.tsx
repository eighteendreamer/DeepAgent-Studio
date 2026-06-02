import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import {
  getSessionDetail,
  getSessionConversation,
  listSessions,
  listProjects,
  getActiveProject,
  setActiveProject,
  addProject,
  pickProjectFolder,
  removeProject,
  clearApiKey,
  getSettings,
  runChat,
  resolveApproval,
  stopChat,
  getPlanMode,
  forkSession,
  rewindSession,
  exportTranscript,
} from "./api";
import type { RuntimeEvent } from "./api";
import type {
  ApprovalRequest,
  ChatMessage,
  MessagePart,
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
import { AutomationView } from "./components/AutomationView";
import { OnboardingWizard } from "./components/OnboardingWizard";
import { SettingsSidebar } from "./components/SettingsSidebar";
import { SettingsView } from "./components/SettingsView";
import { message } from "./components/message";

type View = "start" | "chat" | "skills" | "knowledge" | "automation" | "settings";

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
  // True while an agent run is streaming. Used to gate new submissions (you
  // can't send a new message mid-output) and to show a busy send button. The
  // run itself is async on the backend, so the UI never freezes — approvals,
  // scrolling and navigation stay responsive while this is true.
  const [isRunning, setIsRunning] = useState(false);
  const [planMode, setPlanMode] = useState(false);
  // The session id of the in-flight run, set as soon as the backend announces
  // it (session_registered) — used for the manual stop button and to navigate
  // into the still-running session.
  const [runningSessionId, setRunningSessionId] = useState<string | null>(null);

  // The in-flight transcript, kept per-session in a ref so it SURVIVES
  // navigation. Leaving and returning to a running session restores its live
  // messages from here (not a lossy DB/timeline reload). Keyed by session id;
  // a not-yet-registered new run uses the "__pending__" key until its id is
  // known, then it is migrated.
  const liveTranscripts = useRef<Map<string, ChatMessage[]>>(new Map());
  // Always-current activeId, readable from inside the long-lived run handler.
  const activeIdRef = useRef<string | null>(null);
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
        const mapped: ChatMessage[] = conv.map((m) => ({
          role: m.role,
          content: m.content,
          usage: m.usage
            ? {
                promptTokens: m.usage.prompt_tokens,
                completionTokens: m.usage.completion_tokens,
                totalTokens: m.usage.total_tokens,
                cacheHitTokens: m.usage.prompt_cache_hit_tokens,
                cacheMissTokens: m.usage.prompt_cache_miss_tokens,
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
                },
              };
            }
            if (p.kind === "reasoning") return { kind: "reasoning", text: p.text };
            return { kind: "text", text: p.text };
          }),
        }));
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
      // Block new submissions while a run is streaming — you can't start a new
      // message until the current output finishes.
      if (isRunning) return;
      setIsRunning(true);

      // Wall-clock start of this run, for the "total time" footer metric.
      const runStart = Date.now();

      // Continue the current session when submitting a follow-up from within an
      // open chat; start a fresh session when submitting from the start view.
      const continueId = view === "chat" && activeId ? activeId : null;

      // The run's session key for the live transcript. Until the backend
      // announces the real id (session_registered), a new run buffers under a
      // pending key; continuations use the existing id directly.
      const PENDING = "__pending__";
      let runKey = continueId ?? PENDING;

      // Seed the transcript: continuations append to the existing one; a fresh
      // run starts clean. Stored in the ref so it survives navigation.
      const prior: ChatMessage[] = continueId
        ? liveTranscripts.current.get(continueId) ?? messagesRef.current
        : [];
      const seeded: ChatMessage[] = [
        ...prior,
        { role: "user", content: text },
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
        if (activeIdRef.current === runKey) {
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
      const summarize = (output: unknown): string => {
        if (output == null) return "";
        if (typeof output === "string") return output.slice(0, 200);
        try {
          const o = output as Record<string, unknown>;
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
              setRunningSessionId(sid);
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
              detail: summarize(event.output),
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
        text,
        onEvent,
        (request) => {
          // Queue the approval request; the dialog shows the head of the queue.
          setApprovals((prev) => [...prev, request]);
        },
        continueId
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
          // Re-enable input once the run finishes (success or error), and clear
          // any approval requests still queued for this finished run.
          setIsRunning(false);
          setRunningSessionId(null);
          setApprovals([]);
          // Drop the live buffer for the finished session: future visits load
          // the authoritative styled conversation from the DB. Keep the pending
          // key clean too.
          liveTranscripts.current.delete(runKey);
          liveTranscripts.current.delete(PENDING);
        });
    },
    [activeId, view, refreshSessions, isRunning]
  );

  const chatMessages = messages;

  // Manually stop the in-flight run (the run ends cleanly at the next step).
  const onStopRun = useCallback(() => {
    const sid = runningSessionId ?? activeId;
    if (sid) stopChat(sid).catch(() => {});
  }, [runningSessionId, activeId]);

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
                onOpenKnowledge={() => navigateTo(activeId, "knowledge")}
                onOpenAutomation={() => navigateTo(activeId, "automation")}
                onOpenSettings={() => navigateTo(activeId, "settings")}
                onLogout={onLogout}
                runningSessionId={isRunning ? runningSessionId : null}
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
                  approval={approvals[0] ?? null}
                  approvalQueueCount={approvals.length}
                  onApprovalDecision={onApprovalDecision}
                  busy={isRunning}
                  onStop={onStopRun}
                  planMode={planMode}
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
