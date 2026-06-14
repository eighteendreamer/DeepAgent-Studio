// API bridge to the Rust kernel via Tauri commands.
//
// When running inside Tauri, calls go through `invoke` to the Rust
// `deepagent-app-core::AppService`. When running in a plain browser (e.g.
// `vite dev` without Tauri, or the smoke build), it falls back to deterministic
// mock data so the UI is always runnable and the build never breaks.

import type {
  ApprovalRequest,
  ArchiveProjectResult,
  ArchivedConversation,
  Balance,
  Command,
  ConversationMessage,
  CostSummary,
  DiagnosticResult,
  DiffResult,
  ForkResult,
  KnowledgeDraft,
  KnowledgeEntry,
  KnowledgeHit,
  McpServer,
  PermissionRules,
  Project,
  ProjectMapHit,
  ProjectMapGraph,
  ProjectMapImpact,
  ProjectMapNeighbors,
  ProjectMapNode,
  ProjectMapOverview,
  ProjectMapRefresh,
  ProjectMapStatus,
  RewindResult,
  SessionDetail,
  SessionSummary,
  SettingsView,
  Skill,
  SkillActivation,
  Transcript,
  WorkspaceInfo,
} from "./types";

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

export const SETTINGS_CHANGED_EVENT = "deepagent:settings-changed";
export const ARCHIVE_CHANGED_EVENT = "deepagent:archive-changed";

export type SandboxMode = "read_only" | "workspace_write" | "full_access";

function emitSettingsChanged(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(SETTINGS_CHANGED_EVENT));
}

function emitArchiveChanged(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent(ARCHIVE_CHANGED_EVENT));
}

function promptMayChangeSettings(prompt: string): boolean {
  const normalized = prompt.trim().toLowerCase();
  return /^\/(model|thinking|effort)(?:\s|$)/.test(normalized);
}

function getInvoke(): InvokeFn | null {
  // Tauri injects __TAURI_INTERNALS__ / the api module at runtime.
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
  if (typeof window !== "undefined" && w.__TAURI_INTERNALS__) {
    // Lazy import so the browser build does not hard-require the module.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return (async (cmd: string, args?: Record<string, unknown>) => {
      const mod = await import("@tauri-apps/api/core");
      return mod.invoke(cmd, args);
    }) as InvokeFn;
  }
  return null;
}

export async function listSessions(): Promise<SessionSummary[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<SessionSummary[]>("list_sessions");
  return [];
}

export async function getSessionDetail(id: string): Promise<SessionDetail> {
  const invoke = getInvoke();
  if (invoke) return invoke<SessionDetail>("session_detail", { sessionId: id });
  throw new Error("session detail requires the desktop app");
}

/** Reconstruct a session's styled conversation (messages with ordered parts:
 * reasoning / tool cards / text), for replaying a returned-to session. */
export async function getSessionConversation(id: string): Promise<ConversationMessage[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<ConversationMessage[]>("session_conversation", { sessionId: id });
  return [];
}

export async function setSessionPinned(sessionId: string, pinned: boolean): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("set_session_pinned", { sessionId, pinned });
  return pinned;
}

export async function getCommands(query: string): Promise<Command[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<Command[]>("commands", { query });
  return filterMock(query);
}

export async function computeDiff(oldText: string, newText: string): Promise<DiffResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<DiffResult>("compute_diff", { old: oldText, new: newText });
  return mockDiff(oldText, newText);
}

// ---- settings / project initialization ------------------------------------

/**
 * Initialize the project with a DeepSeek API key. This **validates** the key by
 * running model discovery against DeepSeek — an invalid key rejects (throws).
 * On success the key is stored in the OS keychain and the redacted view is
 * returned. Only callable inside the desktop app.
 */
export async function initializeProject(apiKey: string): Promise<SettingsView> {
  const invoke = getInvoke();
  if (invoke) {
    const view = await invoke<SettingsView>("initialize_project", { apiKey });
    emitSettingsChanged();
    return view;
  }
  throw new Error("connecting an API key requires the desktop app");
}

/** The current (redacted) settings view, or null if uninitialized. */
export async function getSettings(): Promise<SettingsView | null> {
  const invoke = getInvoke();
  if (invoke) return invoke<SettingsView | null>("get_settings");
  return null;
}

/** Re-run model discovery with the stored key. */
export async function refreshModels(): Promise<SettingsView> {
  const invoke = getInvoke();
  if (invoke) {
    const view = await invoke<SettingsView>("refresh_models");
    emitSettingsChanged();
    return view;
  }
  throw new Error("refresh requires the desktop app");
}

/**
 * Fetch the user's DeepSeek account balance (CNY, including granted +
 * topped-up portions). Hits the live `GET /user/balance` endpoint with the
 * stored API key. Throws on network/auth errors so the UI can show a clear
 * "—" indicator with the message in a tooltip.
 */
export async function getBalance(): Promise<Balance> {
  const invoke = getInvoke();
  if (invoke) return invoke<Balance>("get_balance");
  return { is_available: false, infos: [] };
}

/** Clear the stored API key (sign out). */
export async function clearApiKey(): Promise<void> {
  const invoke = getInvoke();
  if (invoke) {
    await invoke("clear_api_key");
    emitSettingsChanged();
  }
}

/**
 * Switch the active chat model to a discovered model id. Returns the updated
 * (redacted) settings view. Throws if the id is not in `available_models`.
 */
export async function setChatModel(modelId: string): Promise<SettingsView> {
  const invoke = getInvoke();
  if (invoke) {
    const view = await invoke<SettingsView>("set_chat_model", { modelId });
    emitSettingsChanged();
    return view;
  }
  throw new Error("switching models requires the desktop app");
}

/** Set DeepSeek Thinking Mode depth for subsequent chat requests. */
export async function setThinkingDepth(
  depth: "simple" | "medium" | "deep"
): Promise<SettingsView> {
  const invoke = getInvoke();
  if (invoke) {
    const view = await invoke<SettingsView>("set_thinking_depth", { depth });
    emitSettingsChanged();
    return view;
  }
  throw new Error("changing thinking depth requires the desktop app");
}

/**
 * Fork a session at a timeline sequence: create a new branch copying events
 * `0..=atSeq`. The source session is left untouched. Returns the branch id.
 */
export async function forkSession(id: string, atSeq: number): Promise<ForkResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<ForkResult>("fork_session", { sessionId: id, atSeq });
  return { new_session_id: `${id}-fork`, source_session_id: id, forked_at: atSeq };
}

/**
 * Rewind a session in place to a timeline sequence, discarding later events.
 * Destructive and user-initiated; the session is reopened afterwards.
 */
export async function rewindSession(id: string, toSeq: number): Promise<RewindResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<RewindResult>("rewind_session", { sessionId: id, toSeq });
  return { session_id: id, kept_through: toSeq, events_removed: 0 };
}

/** Export a session transcript as "markdown" or "json". */
export async function exportTranscript(
  id: string,
  format: "markdown" | "json"
): Promise<Transcript> {
  const invoke = getInvoke();
  if (invoke) return invoke<Transcript>("export_transcript", { sessionId: id, format });
  return {
    session_id: id,
    format,
    extension: format === "json" ? "json" : "md",
    content: format === "json" ? "[]" : "# Session transcript\n",
  };
}

/** Pending approvals are pushed by the runtime; mocked here for preview. */
export async function getPendingApprovals(): Promise<ApprovalRequest[]> {
  return mockApprovals();
}

export async function runDoctor(): Promise<DiagnosticResult[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<DiagnosticResult[]>("run_doctor");
  return [
    {
      name: "API Key",
      status: "warning",
      detail: "desktop runtime is unavailable in browser preview",
      fix_hint: "Run the Tauri desktop app to execute diagnostics.",
    },
  ];
}

// ---- skills ---------------------------------------------------------------

export async function listSkills(): Promise<Skill[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<Skill[]>("list_skills");
  return mockSkills();
}

export async function reloadSkills(): Promise<Skill[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<Skill[]>("reload_skills");
  return mockSkills();
}

export async function installSkill(sourceDir: string): Promise<Skill> {
  const invoke = getInvoke();
  if (invoke) return invoke<Skill>("install_skill", { sourceDir });
  throw new Error("install requires the desktop app");
}

export async function installSkillFromZip(zipPath: string): Promise<Skill> {
  const invoke = getInvoke();
  if (invoke) return invoke<Skill>("install_skill_from_zip", { zipPath });
  throw new Error("zip install requires the desktop app");
}

export async function uninstallSkill(id: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("uninstall_skill", { id });
  return false;
}

export async function previewSkillActivation(
  query: string
): Promise<SkillActivation | null> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<SkillActivation | null>("preview_skill_activation", { query });
  return mockPreview(query);
}

export async function activateSkill(id: string): Promise<SkillActivation | null> {
  const invoke = getInvoke();
  if (invoke) return invoke<SkillActivation | null>("activate_skill", { id });
  const s = mockSkills().find((x) => x.id === id);
  return s ? { id, body: `# Skill: ${s.name}\n\n${s.description}` } : null;
}

// ---- knowledge base -------------------------------------------------------

/** List all knowledge entries (sorted by id). */
export async function kbList(): Promise<KnowledgeEntry[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<KnowledgeEntry[]>("kb_list");
  return mockKnowledge();
}

/** Search the knowledge base; returns scored hits (best first). */
export async function kbSearch(
  query: string,
  kind?: string | null,
  limit?: number
): Promise<KnowledgeHit[]> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<KnowledgeHit[]>("kb_search", {
      query,
      kind: kind ?? null,
      limit: limit ?? null,
    });
  return mockKnowledge()
    .filter(
      (e) =>
        e.title.toLowerCase().includes(query.toLowerCase()) ||
        e.body.toLowerCase().includes(query.toLowerCase())
    )
    .filter((e) => !kind || e.kind === kind)
    .slice(0, limit ?? 10)
    .map((e) => ({
      id: e.id,
      title: e.title,
      kind: e.kind,
      scope: e.scope,
      score: 0.5,
      excerpt: e.body.slice(0, 160),
    }));
}

/** Get a single entry by composite id (`scope:slug`). */
export async function kbGet(id: string): Promise<KnowledgeEntry | null> {
  const invoke = getInvoke();
  if (invoke) return invoke<KnowledgeEntry | null>("kb_get", { id });
  return mockKnowledge().find((e) => e.id === id) ?? null;
}

/** Create or update a knowledge entry. */
export async function kbSave(draft: KnowledgeDraft): Promise<KnowledgeEntry> {
  const invoke = getInvoke();
  if (invoke) return invoke<KnowledgeEntry>("kb_save", { draft });
  const now = Date.now();
  return {
    id: `project:${draft.title.toLowerCase().replace(/\s+/g, "-")}`,
    title: draft.title,
    kind: draft.kind ?? "note",
    tags: draft.tags,
    scope: draft.scope ?? "project",
    created_at: now,
    updated_at: now,
    source_session: null,
    body: draft.body,
  };
}

/** Delete a knowledge entry by id. */
export async function kbDelete(id: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("kb_delete", { id });
  return true;
}

/** Re-scan vaults from disk and return the refreshed list. */
export async function kbReload(): Promise<KnowledgeEntry[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<KnowledgeEntry[]>("kb_reload");
  return mockKnowledge();
}

/** Toggle passive injection; returns the new state. */
export async function kbSetPassive(enabled: boolean): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("kb_set_passive", { enabled });
  return enabled;
}

/** Whether passive injection is currently enabled. */
export async function kbPassiveEnabled(): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("kb_passive_enabled");
  return true;
}

/** List pending auto-capture drafts awaiting confirmation. */
export async function kbListDrafts(): Promise<KnowledgeEntry[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<KnowledgeEntry[]>("kb_list_drafts");
  return [];
}

/** Accept a draft (promote to an active entry). */
export async function kbAcceptDraft(id: string): Promise<KnowledgeEntry> {
  const invoke = getInvoke();
  if (invoke) return invoke<KnowledgeEntry>("kb_accept_draft", { id });
  throw new Error("accepting a draft requires the desktop app");
}

/** Discard a draft. */
export async function kbDiscardDraft(id: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("kb_discard_draft", { id });
  return true;
}

/** Toggle session auto-capture; returns the new state. */
export async function kbSetAutoCapture(enabled: boolean): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("kb_set_auto_capture", { enabled });
  return enabled;
}

/** Whether session auto-capture is currently enabled. */
export async function kbAutoCaptureEnabled(): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("kb_auto_capture_enabled");
  return true;
}

// ---- cost tracking + budget -----------------------------------------------

const EMPTY_COST_SUMMARY: CostSummary = {
  session_cost: 0,
  today_cost: 0,
  month_cost: 0,
  total_cost: 0,
  currency: "USD",
  budget: { daily_limit: null, monthly_limit: null },
};

/** Accumulated cost summary (session / today / month / total + budget). */
export async function getCostSummary(sessionId?: string): Promise<CostSummary> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<CostSummary>("get_cost_summary", { sessionId: sessionId ?? null });
  return EMPTY_COST_SUMMARY;
}

/** Set the daily/monthly budget (USD); returns the refreshed summary. */
export async function setBudget(
  dailyLimit: number | null,
  monthlyLimit: number | null,
): Promise<CostSummary> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<CostSummary>("set_budget", {
      dailyLimit,
      monthlyLimit,
    });
  return EMPTY_COST_SUMMARY;
}

// ---- chat (streamed) ------------------------------------------------------

/** A live runtime event mirroring deepagent-runtime::RuntimeEvent. */
export interface RuntimeEvent {
  type:
    | "run_started"
    | "session_registered"
    | "turn_started"
    | "reasoning_delta"
    | "content_delta"
    | "tool_started"
    | "tool_completed"
    | "tool_blocked"
    | "verification"
    | "usage"
    | "run_completed"
    | "run_awaiting_approval"
    | "run_failed"
    | "run_cancelled";
  // Fields are variant-specific (tagged union); read what each type carries.
  [key: string]: unknown;
}

interface RunEventEnvelope<T> {
  run_id: string;
  payload: T;
}

function randomRunId(): string {
  const cryptoApi = globalThis.crypto;
  if (cryptoApi?.randomUUID) return cryptoApi.randomUUID();
  return `run_${Date.now()}_${Math.random().toString(16).slice(2)}`;
}

function unwrapRunPayload<T>(
  payload: T | RunEventEnvelope<T>,
  runId: string
): T | null {
  const maybe = payload as RunEventEnvelope<T>;
  if (maybe && typeof maybe === "object" && "run_id" in maybe && "payload" in maybe) {
    return maybe.run_id === runId ? maybe.payload : null;
  }
  // Browser/mock fallback and older desktop builds emitted the raw payload.
  return payload as T;
}

/**
 * Run a streamed chat turn-loop. Runtime events go to `onEvent`; any tool
 * approval request goes to `onApproval` (the UI shows a dialog and later calls
 * `resolveApproval`). When `sessionId` is given, the turn **continues** that
 * session (its prior conversation is replayed to the model); otherwise a new
 * session is created. Resolves with the session id used.
 */
export async function runChat(
  prompt: string,
  onEvent: (event: RuntimeEvent) => void,
  onApproval?: (request: ApprovalRequest) => void,
  sessionId?: string | null,
  runId?: string
): Promise<string> {
  const invoke = getInvoke();
  if (invoke) {
    const mod = await import("@tauri-apps/api/event");
    const actualRunId = runId ?? randomRunId();
    const unlistenEvent = await mod.listen<RunEventEnvelope<RuntimeEvent> | RuntimeEvent>("chat://event", (e) => {
      const payload = unwrapRunPayload(e.payload, actualRunId);
      if (payload) onEvent(payload);
    });
    const unlistenApproval = await mod.listen<RunEventEnvelope<ApprovalRequest> | ApprovalRequest>(
      "chat://approval",
      (e) => {
        const payload = unwrapRunPayload(e.payload, actualRunId);
        if (payload) onApproval?.({ ...payload, run_id: actualRunId });
      }
    );
    try {
      const nextSessionId = await invoke<string>("run_chat", { prompt, sessionId: sessionId ?? null, runId: actualRunId });
      if (promptMayChangeSettings(prompt)) emitSettingsChanged();
      return nextSessionId;
    } finally {
      unlistenEvent();
      unlistenApproval();
    }
  }
  return mockChatStream(prompt, onEvent);
}

/** Resolve a pending tool approval (true = approved). */
export async function resolveApproval(
  callId: string,
  approved: boolean
): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("resolve_approval", { callId, approved });
  return true;
}

/** Request a manual stop of an in-flight run for `sessionId`. */
export async function stopChat(sessionId: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("stop_chat", { sessionId });
  return false;
}

/** Whether a session is currently in read-only Plan mode. */
export async function getPlanMode(sessionId: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("get_plan_mode", { sessionId });
  return false;
}

/** Set a session's read-only Plan mode state. */
export async function setPlanMode(sessionId: string, active: boolean): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("set_plan_mode", { sessionId, active });
  return active;
}

/** Get the current approval policy label. */
export async function getApprovalPolicy(): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("get_approval_policy");
  return "always_ask";
}

/** Set the approval policy ("always_ask" | "auto_review" | "full_access"). */
export async function setApprovalPolicy(policy: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("set_approval_policy", { policy });
}

/** Get the current sandbox mode label. */
export async function getSandboxMode(): Promise<SandboxMode> {
  const invoke = getInvoke();
  if (invoke) return invoke<SandboxMode>("get_sandbox_mode");
  return "workspace_write";
}

/** Set the sandbox mode ("read_only" | "workspace_write" | "full_access"). */
export async function setSandboxMode(mode: SandboxMode): Promise<SettingsView> {
  const invoke = getInvoke();
  if (invoke) {
    const view = await invoke<SettingsView>("set_sandbox_mode", { mode });
    emitSettingsChanged();
    return view;
  }
  throw new Error("changing sandbox mode requires the desktop app");
}

// ---- tool-search lazy loading (tool-search spec) --------------------------

export type ToolSearchMode = "disabled" | "enabled" | "auto";

/** Get the current tool-search mode label. Default `disabled` outside Tauri. */
export async function getToolSearchMode(): Promise<ToolSearchMode> {
  const invoke = getInvoke();
  if (invoke) return invoke<ToolSearchMode>("get_tool_search_mode");
  return "disabled";
}

/** Set the tool-search mode. Returns the persisted label on success. */
export async function setToolSearchMode(
  mode: ToolSearchMode,
): Promise<ToolSearchMode> {
  const invoke = getInvoke();
  if (invoke) {
    const label = await invoke<ToolSearchMode>("set_tool_search_mode", { mode });
    emitSettingsChanged();
    return label;
  }
  throw new Error("changing tool-search mode requires the desktop app");
}

/** The current Auto-mode threshold in characters. Falls back to the
 *  backend default (8000) when no override is persisted. */
export async function getToolSearchThreshold(): Promise<number> {
  const invoke = getInvoke();
  if (invoke) return invoke<number>("get_tool_search_threshold");
  return 8000;
}

/** Persist the Auto-mode threshold. Pass `null` to revert to the default. */
export async function setToolSearchThreshold(
  value: number | null,
): Promise<number> {
  const invoke = getInvoke();
  if (invoke) {
    const v = await invoke<number>("set_tool_search_threshold", {
      value: value === null || value === undefined ? null : Math.max(1, Math.floor(value)),
    });
    emitSettingsChanged();
    return v;
  }
  throw new Error("changing tool-search threshold requires the desktop app");
}

// ---- MCP servers (visual config) ------------------------------------------

export async function listMcpServers(): Promise<McpServer[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<McpServer[]>("list_mcp_servers");
  return mockMcpServers();
}

export async function saveMcpServer(server: McpServer): Promise<McpServer> {
  const invoke = getInvoke();
  if (invoke) return invoke<McpServer>("save_mcp_server", { server });
  return server;
}

export async function removeMcpServer(name: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("remove_mcp_server", { name });
  return true;
}

export async function setMcpServerEnabled(
  name: string,
  enabled: boolean
): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("set_mcp_server_enabled", { name, enabled });
  return true;
}

// ---- permission rules (declarative allow/ask/deny) ------------------------

export async function getPermissionRules(): Promise<PermissionRules> {
  const invoke = getInvoke();
  if (invoke) return invoke<PermissionRules>("get_permission_rules");
  return { allow: [], ask: [], deny: [] };
}

export async function setPermissionRules(rules: PermissionRules): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("set_permission_rules", { rules });
}

// ---- declarative external hooks (hooks.json) ------------------------------

/** Get the raw hooks.json source (empty string when unset). */
export async function getHooksJson(): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("get_hooks_json");
  return "";
}

/**
 * Set the hooks.json source. Rejects (throws) with a parse-error message when
 * the JSON is malformed, so the UI can surface validation feedback.
 */
export async function setHooksJson(hooksJson: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("set_hooks_json", { hooksJson });
}

// ---- workspace (active project) -------------------------------------------

/** The active project folder (name + absolute path). */
export async function getWorkspaceInfo(): Promise<WorkspaceInfo> {
  const invoke = getInvoke();
  if (invoke) return invoke<WorkspaceInfo>("workspace_info");
  return { name: "Looker-v2", path: "Looker-v2" };
}

// ---- projects (sidebar: folders → sessions) -------------------------------

/** List all opened projects with session counts. */
export async function listProjects(): Promise<Project[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<Project[]>("list_projects");
  return [];
}

/** The active project path, or null. */
export async function getActiveProject(): Promise<string | null> {
  const invoke = getInvoke();
  if (invoke) return invoke<string | null>("active_project");
  return null;
}

/** Open (add) a project folder; it becomes active. */
export async function addProject(path: string): Promise<Project> {
  const invoke = getInvoke();
  if (invoke) return invoke<Project>("add_project", { path });
  return {
    name: path.split(/[\\/]/).pop() || path,
    path,
    pinned: false,
    session_count: 0,
    updated_at: Date.now(),
  };
}

/** Set the active project. */
export async function setActiveProject(path: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("set_active_project", { path });
}

/** Close (remove) a project from the sidebar. */
export async function removeProject(path: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("remove_project", { path });
  return true;
}

export async function setProjectPinned(path: string, pinned: boolean): Promise<Project> {
  const invoke = getInvoke();
  if (invoke) return invoke<Project>("set_project_pinned", { path, pinned });
  return {
    name: path.split(/[\\/]/).pop() || path,
    path,
    pinned,
    session_count: 0,
    updated_at: Date.now(),
  };
}

export async function renameProject(path: string, name: string): Promise<Project> {
  const invoke = getInvoke();
  if (invoke) return invoke<Project>("rename_project", { path, name });
  return {
    name,
    path,
    pinned: false,
    session_count: 0,
    updated_at: Date.now(),
  };
}

export async function openProjectInFileManager(path: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("open_project_in_file_manager", { path });
}

// ---- project map ----------------------------------------------------------

export async function projectMapStatus(projectPath?: string | null): Promise<ProjectMapStatus> {
  const invoke = getInvoke();
  if (invoke) return invoke<ProjectMapStatus>("project_map_status", { projectPath: projectPath ?? null });
  return mockProjectMapOverview().status;
}

export async function projectMapOverview(projectPath?: string | null): Promise<ProjectMapOverview> {
  const invoke = getInvoke();
  if (invoke) return invoke<ProjectMapOverview>("project_map_overview", { projectPath: projectPath ?? null });
  return mockProjectMapOverview();
}

export async function projectMapSearch(
  query: string,
  limit?: number,
  projectPath?: string | null
): Promise<ProjectMapHit[]> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<ProjectMapHit[]>("project_map_search", {
      projectPath: projectPath ?? null,
      query,
      limit: limit ?? null,
    });
  const q = query.trim().toLowerCase();
  return mockProjectMapHits().filter((h) =>
    !q || `${h.name} ${h.file_path ?? ""} ${h.summary}`.toLowerCase().includes(q)
  ).slice(0, limit ?? 20);
}

export async function projectMapNode(
  nodeId: string,
  projectPath?: string | null
): Promise<ProjectMapNode | null> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<ProjectMapNode | null>("project_map_node", {
      projectPath: projectPath ?? null,
      nodeId,
    });
  const hit = mockProjectMapHits().find((h) => h.node_id === nodeId);
  return hit
    ? {
        id: hit.node_id,
        node_type: hit.node_type,
        name: hit.name,
        file_path: hit.file_path,
        line_range: null,
        summary: hit.summary,
        tags: ["mock"],
        complexity: hit.complexity,
        language_notes: null,
      }
    : null;
}

export async function projectMapNeighbors(
  nodeId: string,
  projectPath?: string | null
): Promise<ProjectMapNeighbors> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<ProjectMapNeighbors>("project_map_neighbors", {
      projectPath: projectPath ?? null,
      nodeId,
    });
  const node = await projectMapNode(nodeId, projectPath);
  const hits = mockProjectMapHits().filter((h) => h.node_id !== nodeId);
  return {
    node,
    imports: hits.slice(0, 2).map((node) => ({ edge_type: "imports", direction: "out", node })),
    imported_by: hits.slice(2, 4).map((node) => ({ edge_type: "imports", direction: "in", node })),
    calls: [],
    called_by: [],
    related: [],
  };
}

export async function projectMapGraph(
  limit?: number,
  projectPath?: string | null
): Promise<ProjectMapGraph> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<ProjectMapGraph>("project_map_graph", {
      projectPath: projectPath ?? null,
      limit: limit ?? null,
    });
  const nodes = mockProjectMapHits();
  return {
    nodes,
    edges: nodes.slice(1).map((node, index) => ({
      source: nodes[0].node_id,
      target: node.node_id,
      edge_type: index % 2 === 0 ? "imports" : "contains",
      weight: 0.7,
    })),
  };
}

export async function projectMapImpact(
  target: string,
  projectPath?: string | null
): Promise<ProjectMapImpact> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<ProjectMapImpact>("project_map_impact", {
      projectPath: projectPath ?? null,
      target,
    });
  return {
    target: await projectMapNode(target, projectPath),
    direct: mockProjectMapHits().slice(0, 3),
    indirect: mockProjectMapHits().slice(3, 6),
  };
}

export async function projectMapRefreshDeep(projectPath?: string | null): Promise<ProjectMapRefresh> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<ProjectMapRefresh>("project_map_refresh_deep", {
      projectPath: projectPath ?? null,
    });
  }
  const overview = mockProjectMapOverview();
  return {
    ok: true,
    graph_path: ".understand-anything/knowledge-graph.json",
    files: overview.status.files,
    nodes: overview.status.nodes,
    edges: overview.status.edges,
    duration_ms: 420,
    truncated: false,
    message: "Understand-Anything 深度项目地图已生成。",
    status: { ...overview.status, source: "understand-anything" },
  };
}

export async function archiveProjectConversations(
  projectPath: string
): Promise<ArchiveProjectResult> {
  const invoke = getInvoke();
  if (invoke) {
    const result = await invoke<ArchiveProjectResult>("archive_project_conversations", {
      projectPath,
    });
    emitArchiveChanged();
    return result;
  }
  emitArchiveChanged();
  return {
    project_path: projectPath,
    project_name: projectPath.split(/[\\/]/).pop() || projectPath,
    archived_count: 0,
  };
}

export async function archiveConversation(sessionId: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) {
    const result = await invoke<boolean>("archive_conversation", { sessionId });
    emitArchiveChanged();
    return result;
  }
  emitArchiveChanged();
  return true;
}

export async function archiveAllConversations(): Promise<number> {
  const invoke = getInvoke();
  if (invoke) {
    const result = await invoke<number>("archive_all_conversations");
    emitArchiveChanged();
    return result;
  }
  emitArchiveChanged();
  return 0;
}

export async function listArchivedConversations(): Promise<ArchivedConversation[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<ArchivedConversation[]>("list_archived_conversations");
  return [];
}

export async function unarchiveConversation(sessionId: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) {
    const result = await invoke<boolean>("unarchive_conversation", { sessionId });
    if (result) emitArchiveChanged();
    return result;
  }
  emitArchiveChanged();
  return true;
}

export async function deleteArchivedConversation(sessionId: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) {
    const result = await invoke<boolean>("delete_archived_conversation", { sessionId });
    if (result) emitArchiveChanged();
    return result;
  }
  emitArchiveChanged();
  return true;
}

export async function deleteAllArchivedConversations(): Promise<number> {
  const invoke = getInvoke();
  if (invoke) {
    const result = await invoke<number>("delete_all_archived_conversations");
    if (result > 0) emitArchiveChanged();
    return result;
  }
  emitArchiveChanged();
  return 0;
}

// ---- terminal (run commands in the active project) ------------------------

/** Result of a one-shot terminal command (mirrors TerminalResultDto). */
export interface TerminalResult {
  command: string;
  cwd: string;
  exit_code: number | null;
  stdout: string;
  stderr: string;
  blocked: boolean;
}

/** Run a shell command in the active project directory. */
export async function runTerminal(command: string): Promise<TerminalResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<TerminalResult>("run_terminal", { command });
  return {
    command,
    cwd: "",
    exit_code: null,
    stdout: "",
    stderr: "terminal requires the desktop app",
    blocked: false,
  };
}

/** The current working directory for the terminal (active project root). */
export async function terminalCwd(): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("terminal_cwd");
  return "";
}

/**
 * Open the OS-native "select folder" dialog and return the chosen absolute
 * path, or null if the user cancelled. Only available inside the desktop app.
 */
export async function pickProjectFolder(): Promise<string | null> {
  if (!isTauri()) return null;
  const mod = await import("@tauri-apps/plugin-dialog");
  const selected = await mod.open({
    directory: true,
    multiple: false,
    title: "Select Project Root",
  });
  if (typeof selected === "string") return selected;
  return null;
}

function mockMcpServers(): McpServer[] {
  return [
    {
      name: "node_repl",
      transport: "stdio",
      enabled: true,
      command: "node",
      args: ["--experimental-repl-await"],
      env: {},
      url: null,
      headers: {},
    },
  ];
}

function mockProjectMapHits(): ProjectMapHit[] {
  return [
    {
      node_id: "file:apps/desktop/src/App.tsx",
      node_type: "file",
      name: "App.tsx",
      file_path: "apps/desktop/src/App.tsx",
      summary: "Application shell, project/session state, and main view routing.",
      complexity: "complex",
      score: 0.95,
    },
    {
      node_id: "file:apps/desktop/src/components/ChatView.tsx",
      node_type: "file",
      name: "ChatView.tsx",
      file_path: "apps/desktop/src/components/ChatView.tsx",
      summary: "Conversation view, composer, tool cards, and side panels.",
      complexity: "complex",
      score: 0.9,
    },
    {
      node_id: "file:crates/deepagent-app-core/src/chat_service.rs",
      node_type: "file",
      name: "chat_service.rs",
      file_path: "crates/deepagent-app-core/src/chat_service.rs",
      summary: "Runs streamed chat sessions and assembles the tool registry.",
      complexity: "complex",
      score: 0.88,
    },
    {
      node_id: "file:crates/deepagent-builtins/src/file_tools.rs",
      node_type: "file",
      name: "file_tools.rs",
      file_path: "crates/deepagent-builtins/src/file_tools.rs",
      summary: "Workspace-confined read/write/edit/list/glob/grep tools.",
      complexity: "moderate",
      score: 0.76,
    },
  ];
}

function mockProjectMapOverview(): ProjectMapOverview {
  return {
    status: {
      status: "ready",
      source: "mock",
      graph_path: null,
      updated_at: Date.now(),
      nodes: 240,
      edges: 510,
      files: 92,
      functions: 118,
      classes: 30,
      last_error: null,
    },
    project_name: "DeepAgent-Studio",
    description: "Mock project map preview.",
    languages: ["typescript", "rust"],
    frameworks: ["react", "tauri"],
    complex_nodes: mockProjectMapHits(),
  };
}

async function mockChatStream(
  prompt: string,
  onEvent: (event: RuntimeEvent) => void
): Promise<string> {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  onEvent({ type: "run_started", task_id: "mock" });
  onEvent({ type: "turn_started", step: 0 });
  const reply = `I received: "${prompt}". (Connect a DeepSeek API key in 设置 for live responses.)`;
  for (const word of reply.split(" ")) {
    await sleep(35);
    onEvent({ type: "content_delta", text: word + " " });
  }
  onEvent({ type: "run_completed", message: reply });
  return "ses_mock0000000000000000000000000001";
}

export function isTauri(): boolean {
  return getInvoke() !== null;
}

// ---- Mock data (browser/dev fallback) ------------------------------------

const MOCK_COMMANDS: Command[] = [
  { id: "session.new", title: "New Session", description: "Start a fresh conversation.", category: "Session", shortcut: "Ctrl+N" },
  { id: "session.end", title: "End Session", description: "Mark the current session as ended.", category: "Session", shortcut: null },
  { id: "session.refresh", title: "Refresh Sessions", description: "Reload the session list.", category: "Session", shortcut: "Ctrl+R" },
  { id: "view.timeline", title: "Show Timeline", description: "Open the event timeline for the current session.", category: "View", shortcut: "Ctrl+1" },
  { id: "view.metrics", title: "Toggle Metrics Panel", description: "Show or hide session metrics.", category: "View", shortcut: "Ctrl+2" },
  { id: "view.diff", title: "Open Diff View", description: "Open the text diff tool.", category: "View", shortcut: "Ctrl+D" },
  { id: "approvals.review", title: "Review Pending Approvals", description: "Open pending tool approval requests.", category: "Approvals", shortcut: "Ctrl+Shift+A" },
  { id: "mcp.list", title: "List MCP Servers", description: "Open the configured MCP server list.", category: "MCP", shortcut: null },
  { id: "theme.toggle", title: "Toggle Theme", description: "Switch between light and dark UI themes.", category: "View", shortcut: null },
  { id: "slash.compact", title: "/compact", description: "压缩当前会话上下文，降低后续请求的上下文体积", category: "内置命令", shortcut: null },
  { id: "slash.cost", title: "/cost", description: "查看当前会话、当天、本月和累计费用", category: "内置命令", shortcut: null },
  { id: "slash.doctor", title: "/doctor", description: "运行环境诊断，检查配置、数据库、权限和 API Key", category: "内置命令", shortcut: null },
  { id: "slash.help", title: "/help", description: "查看可用的 slash 命令列表", category: "内置命令", shortcut: null },
  { id: "slash.status", title: "/status", description: "查看当前项目、模型、思考深度和运行状态", category: "内置命令", shortcut: null },
  { id: "slash.settings", title: "/settings", description: "查看 DeepSeek 配置、模型、权限和思考设置", category: "内置命令", shortcut: null },
  { id: "slash.config", title: "/config", description: "/settings 的别名，查看当前配置摘要", category: "内置命令", shortcut: null },
  { id: "slash.permissions", title: "/permissions", description: "查看当前工具权限策略和 allow/ask/deny 规则", category: "内置命令", shortcut: null },
  { id: "slash.knowledge", title: "/knowledge", description: "查看项目知识库、草稿、被动注入和自动捕获状态", category: "内置命令", shortcut: null },
  { id: "slash.memory", title: "/memory", description: "/knowledge 的别名，查看知识库状态", category: "内置命令", shortcut: null },
  { id: "slash.mcp", title: "/mcp", description: "查看已配置的 MCP 服务及启用状态", category: "内置命令", shortcut: null },
  { id: "slash.projects", title: "/projects", description: "查看已打开项目和当前激活项目", category: "内置命令", shortcut: null },
  { id: "slash.sessions", title: "/sessions", description: "查看最近会话列表", category: "内置命令", shortcut: null },
  { id: "slash.thinking", title: "/thinking", description: "查看或设置思考深度：simple、medium、deep", category: "内置命令", shortcut: null },
  { id: "slash.effort", title: "/effort", description: "/thinking 的别名，查看或设置思考深度", category: "内置命令", shortcut: null },
  { id: "slash.plan", title: "/plan", description: "进入只读 Plan 模式，先规划再执行", category: "内置命令", shortcut: null },
  { id: "slash.execute", title: "/execute", description: "退出 Plan 模式，恢复正常执行权限", category: "内置命令", shortcut: null },
  { id: "slash.resume", title: "/resume", description: "按会话 ID 恢复历史会话上下文", category: "内置命令", shortcut: null },
  { id: "slash.model", title: "/model", description: "切换当前聊天模型，例如 /model deepseek-v4-pro", category: "内置命令", shortcut: null },
  { id: "slash.clear", title: "/clear", description: "清空当前聊天输入界面提示", category: "内置命令", shortcut: null },
];

function isSubsequence(needle: string, haystack: string): boolean {
  let i = 0;
  for (const hc of haystack) {
    if (i >= needle.length) break;
    if (needle[i] === hc) i++;
  }
  return i >= needle.length;
}

function filterMock(query: string): Command[] {
  const q = query.trim().toLowerCase().replace(/\s+/g, "");
  if (!q) return MOCK_COMMANDS;
  return MOCK_COMMANDS.filter((c) =>
    isSubsequence(q, `${c.title} ${c.category}`.toLowerCase())
  );
}

function mockApprovals(): ApprovalRequest[] {
  return [
    {
      call_id: "call_demo_1",
      tool: "shell",
      risk: "high",
      arguments: '{\n  "cmd": "rm -rf ./build"\n}',
      reason: "high-risk tool requires explicit approval",
    },
  ];
}

// Mirrors the skills auto-discovered from `.deepagent/skills/` so the browser
// preview matches what the desktop app shows.
function mockSkills(): Skill[] {
  return [
    { id: "agent-browser", name: "Agent Browser", description: "Browser automation via a CLI using accessibility-tree element refs — navigate, fill forms, click, screenshot, scrape, test web apps.", version: "0.1.0", origin: "workspace", triggers: ["browse a website", "fill a form", "click a button", "take a screenshot", "scrape a page", "extract data from a web page", "test a web app", "web automation"] },
    { id: "code-review-skill", name: "Code Review", description: "Structured code review with severity-classified findings across correctness, security, performance, and maintainability.", version: "0.1.0", origin: "workspace", triggers: ["review code", "review a file", "review a diff", "review a pull request", "audit code quality", "find security issues", "check for performance problems"] },
    { id: "mcp-builder", name: "MCP Builder", description: "Guide for creating high-quality MCP servers that let LLMs interact with external services through well-designed tools.", version: "0.1.0", origin: "workspace", triggers: ["build an mcp server", "create a model context protocol server", "define mcp tools", "write mcp evaluations"] },
    { id: "planning-with-files", name: "Planning With Files", description: "Manus-style persistent markdown planning — task_plan.md, notes.md, deliverable — so working state survives context resets.", version: "0.1.0", origin: "workspace", triggers: ["complex task", "multi-step project", "research task", "planning", "organizing work", "tracking progress"] },
    { id: "rust-backend-review", name: "Rust Backend Review", description: "Focused Rust backend review: ownership, error types, async correctness, unsafe, clippy.", version: "0.1.0", origin: "workspace", triggers: ["review rust code", "audit a rust crate", "check error handling", "review unsafe code"] },
    { id: "superpowers", name: "Superpowers", description: "Meta-skill for authoring high-quality skills and following disciplined engineering workflows (brainstorm, plan, TDD).", version: "0.1.0", origin: "workspace", triggers: ["write a skill", "create a new skill", "improve a skill"] },
    { id: "ui-ux-pro-max-skill", name: "UI UX Pro Max", description: "UI/UX design intelligence — styles, color palettes, font pairings, product types, UX guidelines, chart types.", version: "0.1.0", origin: "workspace", triggers: ["design a UI", "improve the UX", "pick a color palette", "choose font pairings", "design a landing page", "audit a UI", "build a dashboard"] },
    { id: "webapp-testing", name: "Webapp Testing", description: "Python Playwright toolkit for testing/automating local web apps — e2e tests, screenshots, console logs, UI debugging.", version: "0.1.0", origin: "workspace", triggers: ["test a web app", "write an end-to-end test", "verify frontend behavior", "capture a screenshot", "debug UI interactions", "automate the browser", "check browser console logs"] },
  ];
}

// Mirrors the knowledge entries so the browser preview has something to show.
function mockKnowledge(): KnowledgeEntry[] {
  const now = Date.now();
  return [
    {
      id: "project:powershell-pipe-interrupt",
      title: "PowerShell 管道命令被 ^C 中断",
      kind: "pitfall",
      tags: ["windows", "powershell", "cargo"],
      scope: "project",
      created_at: now,
      updated_at: now,
      source_session: null,
      body: "## 现象\n`cargo test | Select-String` 经常 exit -1，是 UI artifact 不是真失败。\n\n## 解决\n改用 `> out.txt 2>&1` 重定向后再读。",
    },
    {
      id: "global:cargo-offline-tests",
      title: "离线运行 workspace 测试",
      kind: "command",
      tags: ["cargo", "test"],
      scope: "global",
      created_at: now,
      updated_at: now,
      source_session: null,
      body: "运行后端测试：`cargo test --workspace --offline`。",
    },
  ];
}

function mockPreview(query: string): SkillActivation | null {
  const q = query.toLowerCase();
  const skills = mockSkills();
  let best: { skill: Skill; score: number } | null = null;
  for (const skill of skills) {
    let score = 0;
    for (const t of skill.triggers) {
      if (q.includes(t.toLowerCase())) score += 5 + t.split(/\s+/).length;
    }
    if (score > 0 && (!best || score > best.score)) best = { skill, score };
  }
  return best
    ? { id: best.skill.id, body: `# Skill: ${best.skill.name}\n\n${best.skill.description}` }
    : null;
}

// A tiny LCS diff mirroring the Rust implementation, for browser preview.
function mockDiff(oldText: string, newText: string): DiffResult {
  const a = oldText === "" ? [] : oldText.split("\n");
  const b = newText === "" ? [] : newText.split("\n");
  const n = a.length;
  const m = b.length;
  const t: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = 1; i <= n; i++)
    for (let j = 1; j <= m; j++)
      t[i][j] = a[i - 1] === b[j - 1] ? t[i - 1][j - 1] + 1 : Math.max(t[i - 1][j], t[i][j - 1]);
  const lines: DiffResult["lines"] = [];
  let added = 0;
  let removed = 0;
  let i = n;
  let j = m;
  const rev: DiffResult["lines"] = [];
  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && a[i - 1] === b[j - 1]) {
      rev.push({ kind: "context", old_line: i, new_line: j, content: a[i - 1] });
      i--;
      j--;
    } else if (j > 0 && (i === 0 || t[i][j - 1] >= t[i - 1][j])) {
      rev.push({ kind: "added", old_line: null, new_line: j, content: b[j - 1] });
      added++;
      j--;
    } else {
      rev.push({ kind: "removed", old_line: i, new_line: null, content: a[i - 1] });
      removed++;
      i--;
    }
  }
  rev.reverse();
  lines.push(...rev);
  return { lines, added, removed };
}
