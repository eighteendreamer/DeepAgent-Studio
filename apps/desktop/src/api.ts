// API bridge to the Rust kernel via Tauri commands.
//
// When running inside Tauri, calls go through `invoke` to the Rust
// `deepagent-app-core::AppService`. When running in a plain browser (e.g.
// `vite dev` without Tauri, or the smoke build), it falls back to deterministic
// mock data so the UI is always runnable and the build never breaks.

import type {
  ApiKeyInfo,
  AnySearchApiKeyInfo,
  AnySearchTestResult,
  ApprovalRequest,
  ArchiveProjectResult,
  ArchivedConversation,
  AttachmentIngestInput,
  Balance,
  Command,
  ConversationMessage,
  CostSummary,
  DiagnosticResult,
  DiffResult,
  ForkResult,
  GitBatchCommitPreviewItem,
  GitBatchCommitTarget,
  GitBatchProjectResult,
  GitBranch,
  GitChanges,
  GitCommitMessageDraft,
  GitDiff,
  GitLogEntry,
  GitOperationResult,
  GitProjectStatus,
  GitPushPreview,
  GitPushRiskScan,
  GitRefCompare,
  GitWorktree,
  KnowledgeDraft,
  KnowledgeEntry,
  KnowledgeHit,
  MarketSearchData,
  MarketSearchInput,
  McpConnectionStatus,
  McpServer,
  PermissionPreset,
  PermissionPresetVisibility,
  PermissionRules,
  PersistedAttachment,
  Project,
  ProjectMapHit,
  ProjectMapGraph,
  ProjectMapImpact,
  ProjectMapNeighbors,
  ProjectMapNode,
  ProjectMapOverview,
  ProjectMapRefresh,
  ProjectMapStatus,
  PreviewMetadata,
  ProjectFileListResult,
  ProjectFileSearchResult,
  PreviewResult,
  PdfRenderResult,
  RecordingSession,
  RuntimeProgress,
  RuntimeRoots,
  RuntimeStatus,
  TranscriptSegment,
  VisionRecognizeRequest,
  VisionRecognizeResult,
  VisionSettings,
  RewindResult,
  ScanResult,
  SandboxieStatus,
  SessionDetail,
  SessionSummary,
  SessionUiPrefs,
  SettingsView,
  Skill,
  SkillActivation,
  SkillAiReviewDone,
  SkillAiReviewToken,
  TestKeyResult,
  Transcript,
  WorkspaceInfo,
  WebSearchSettings,
} from "./types";

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

export const SETTINGS_CHANGED_EVENT = "deepagent:settings-changed";
export const ARCHIVE_CHANGED_EVENT = "deepagent:archive-changed";
export const OPEN_AUTOMATION_EVENT = "deepagent:open-automation";
/** Fired by office-agent panels to inject a message into the active chat. */
export const SEND_TO_CHAT_EVENT = "deepagent:send-to-chat";

/** Dispatch text to be sent as a chat message (handled by ChatView). */
export function sendToChat(text: string): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<string>(SEND_TO_CHAT_EVENT, { detail: text }));
}

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

export async function renameSession(sessionId: string, title: string): Promise<SessionSummary> {
  const invoke = getInvoke();
  if (invoke) return invoke<SessionSummary>("rename_session", { sessionId, title });
  return {
    id: sessionId,
    project: undefined,
    title,
    mode: "normal",
    created_at: Date.now(),
    updated_at: Date.now(),
    ended: false,
    pinned: false,
  };
}

export async function getSessionUiPrefs(sessionId: string): Promise<SessionUiPrefs> {
  const invoke = getInvoke();
  if (invoke) return invoke<SessionUiPrefs>("get_session_ui_prefs", { sessionId });
  return { env_panel_auto_open: true };
}

export async function setSessionEnvPanelAutoOpen(
  sessionId: string,
  enabled: boolean,
): Promise<SessionUiPrefs> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<SessionUiPrefs>("set_session_env_panel_auto_open", {
      sessionId,
      enabled,
    });
  }
  return { env_panel_auto_open: enabled };
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

/** Read the user-defined welcome name from the persistent app settings. */
export async function getWelcomeName(): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("get_welcome_name");
  return localStorage.getItem("userName") || "";
}

/** Persist the user-defined welcome name in the desktop app database. */
export async function setWelcomeName(name: string): Promise<string> {
  const normalized = name.trim();
  const invoke = getInvoke();
  if (invoke) return invoke<string>("set_welcome_name", { name: normalized });
  localStorage.setItem("userName", normalized);
  return normalized;
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

/** Persist the preferred integrated-terminal shell. */
export async function setTerminalShell(
  shell: "powershell" | "command_prompt" | "git_bash" | "wsl"
): Promise<SettingsView> {
  const invoke = getInvoke();
  if (invoke) {
    const view = await invoke<SettingsView>("set_terminal_shell", { shell });
    emitSettingsChanged();
    return view;
  }
  throw new Error("changing terminal shell requires the desktop app");
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

/** Persist exported transcript content to a chosen file path. */
export async function saveTranscriptFile(
  transcript: Transcript,
  suggestedName?: string,
): Promise<string | null> {
  const defaultName = suggestedName?.trim() || `session-${transcript.session_id}.${transcript.extension}`;

  if (isTauri()) {
    const dialog = await import("@tauri-apps/plugin-dialog");
    const selected = await dialog.save({
      title: transcript.format === "json" ? "Export Transcript as JSON" : "Export Transcript",
      defaultPath: defaultName,
      filters: [
        {
          name: transcript.format === "json" ? "JSON" : "Markdown",
          extensions: [transcript.extension],
        },
      ],
    });
    if (typeof selected !== "string" || !selected.trim()) return null;
    const invoke = getInvoke();
    if (!invoke) throw new Error("desktop runtime is unavailable");
    await invoke("save_text_file", {
      path: selected,
      content: transcript.content,
    });
    return selected;
  }

  const blob = new Blob([transcript.content], {
    type: transcript.format === "json" ? "application/json" : "text/markdown",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = defaultName;
  a.click();
  URL.revokeObjectURL(url);
  return defaultName;
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

// ---- skill marketplace (skillsmp.com + GitHub install flow) ---------------
//
// Wire-format notes (verified against `apps/desktop/src-tauri/src/lib.rs` and
// the Rust types in `crates/deepagent-skills/`):
//   - Tauri auto-converts snake_case command argument names to camelCase, so
//     `github_url` → `githubUrl`, `temp_id` → `tempId` on the JS call site.
//   - `MarketSearchInput` carries `#[serde(rename_all = "camelCase")]` so its
//     `sortBy` field is camelCase.
//   - Result structs (`ScanResult`, `ApiKeyInfo`, `TestKeyResult`,
//     `AiReviewResult`, `ScanReport`, …) stay snake_case on the wire.

/** `GET /api/v1/skills/search` via the SkillsMP REST client. */
export async function skillMarketSearch(
  input: MarketSearchInput
): Promise<MarketSearchData> {
  const invoke = getInvoke();
  if (invoke) return invoke<MarketSearchData>("skill_market_search", { input });
  return { skills: [], pagination: { page: 1, limit: 20, total: 0, hasNext: false, hasPrev: false } };
}

/** Run a tiny search to exercise the configured SkillsMP API key. */
export async function skillMarketTestKey(): Promise<TestKeyResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<TestKeyResult>("skill_market_test_key");
  return { ok: false, daily_remaining: null, error: "desktop runtime is unavailable" };
}

/** Inspect the current API-key configuration (never returns the key value). */
export async function skillMarketGetApiKey(): Promise<ApiKeyInfo> {
  const invoke = getInvoke();
  if (invoke) return invoke<ApiKeyInfo>("skill_market_get_api_key");
  return { has_user_key: false, source: "none" };
}

/** Save a user-supplied API key to the OS keychain. */
export async function skillMarketSetApiKey(key: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("skill_market_set_api_key", { key });
}

/** Delete the user-supplied API key; the client falls back to the built-in
 *  / anonymous tier. */
export async function skillMarketClearApiKey(): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("skill_market_clear_api_key");
}

/** Download a skill from GitHub via codeload, run the static safety scan,
 *  and stash the unpacked tempdir keyed by an opaque temp-id. */
export async function skillMarketScan(githubUrl: string): Promise<ScanResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<ScanResult>("skill_market_scan", { githubUrl });
  throw new Error("skill marketplace requires the desktop app");
}

/** Subscribe to the streaming AI security review for `tempId`.
 *
 *  Two Tauri events are emitted, both carrying the temp_id in the payload so
 *  multiple concurrent installs can be filtered apart:
 *    - `skill-ai-review`      → one per token (`{ temp_id, token }`)
 *    - `skill-ai-review-done` → one final settle (`{ temp_id, result, error }`)
 *
 *  Returns an unlisten function that stops both listeners. Call it from the
 *  install dialog's cleanup path (cancel / install / unmount). */
export async function skillMarketAiReviewSubscribe(
  tempId: string,
  onToken: (payload: SkillAiReviewToken) => void,
  onDone: (payload: SkillAiReviewDone) => void
): Promise<() => Promise<void>> {
  if (!isTauri()) {
    return async () => {};
  }
  const mod = await import("@tauri-apps/api/event");
  const unlistenToken = await mod.listen<SkillAiReviewToken>(
    "skill-ai-review",
    (event) => {
      if (event.payload && event.payload.temp_id === tempId) onToken(event.payload);
    }
  );
  const unlistenDone = await mod.listen<SkillAiReviewDone>(
    "skill-ai-review-done",
    (event) => {
      if (event.payload && event.payload.temp_id === tempId) onDone(event.payload);
    }
  );
  return async () => {
    unlistenToken();
    unlistenDone();
  };
}

/** Kick off the background AI review task. Tokens / verdict flow through the
 *  events subscribed via {@link skillMarketAiReviewSubscribe}.
 *
 *  Pass `reReview = true` to request a deeper second pass (Medium thinking
 *  budget + 3K reply cap on the backend). The default `false` runs the
 *  faster initial pass (Simple thinking + 2K reply cap). */
export async function skillMarketAiReview(
  tempId: string,
  reReview: boolean = false,
): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("skill_market_ai_review", { tempId, reReview });
}

/** Confirm-and-install: copy the staged tempdir into the marketplace install
 *  root and return the freshly registered skill DTO. */
export async function skillMarketInstall(tempId: string): Promise<Skill> {
  const invoke = getInvoke();
  if (invoke) return invoke<Skill>("skill_market_install", { tempId });
  throw new Error("skill marketplace requires the desktop app");
}

/** Drop the pending scan (and its tempdir on disk). Idempotent — a missing
 *  `tempId` is treated as already-cleared. */
export async function skillMarketCancel(tempId: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("skill_market_cancel", { tempId });
}

// ---- skill marketplace settings (R10) -------------------------------------

/** Whether the auto-activation catalog reminder is injected (R10.1). */
export async function getSkillCatalogEnabled(): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("get_skill_catalog_enabled");
  return true;
}

export async function setSkillCatalogEnabled(enabled: boolean): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) {
    const value = await invoke<boolean>("set_skill_catalog_enabled", { enabled });
    emitSettingsChanged();
    return value;
  }
  return enabled;
}

/** Character budget for the catalog reminder block (R10.2). `0` disables it. */
export async function getSkillCatalogCharBudget(): Promise<number> {
  const invoke = getInvoke();
  if (invoke) return invoke<number>("get_skill_catalog_char_budget");
  return 8000;
}

export async function setSkillCatalogCharBudget(budget: number): Promise<number> {
  const invoke = getInvoke();
  if (invoke) {
    const value = await invoke<number>("set_skill_catalog_char_budget", { budget });
    emitSettingsChanged();
    return value;
  }
  return budget;
}

/** Whether the AI security review runs before install (R10.3). */
export async function getSkillInstallAiReviewEnabled(): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("get_skill_install_ai_review_enabled");
  return true;
}

export async function setSkillInstallAiReviewEnabled(
  enabled: boolean
): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) {
    const value = await invoke<boolean>("set_skill_install_ai_review_enabled", {
      enabled,
    });
    emitSettingsChanged();
    return value;
  }
  return enabled;
}

/** Override model id for the AI review (R10.4). `null` = follow chat model. */
export async function getSkillInstallAiReviewModel(): Promise<string | null> {
  const invoke = getInvoke();
  if (invoke) return invoke<string | null>("get_skill_install_ai_review_model");
  return null;
}

export async function setSkillInstallAiReviewModel(
  model: string | null
): Promise<string | null> {
  const invoke = getInvoke();
  if (invoke) {
    const value = await invoke<string | null>("set_skill_install_ai_review_model", {
      model,
    });
    emitSettingsChanged();
    return value;
  }
  return model;
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
  currency: "CNY",
  budget: { daily_limit: null, monthly_limit: null },
};

/** Accumulated cost summary (session / today / month / total + budget). */
export async function getCostSummary(sessionId?: string): Promise<CostSummary> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<CostSummary>("get_cost_summary", { sessionId: sessionId ?? null });
  return EMPTY_COST_SUMMARY;
}

/** Set the daily/monthly budget (RMB/CNY); returns the refreshed summary. */
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
    | "model_request_started"
    | "model_first_token"
    | "model_request_completed"
    | "reasoning_delta"
    | "content_delta"
    | "tool_started"
    | "tool_completed"
    | "tool_blocked"
    | "verification"
    | "context_usage"
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

export async function sandboxieStatus(): Promise<SandboxieStatus> {
  const invoke = getInvoke();
  if (invoke) return invoke<SandboxieStatus>("sandboxie_status");
  return {
    supported: false,
    ready: false,
    box_name: "DeepAgentStudio",
    message: "Sandboxie-Plus status requires the desktop app",
  };
}

export async function sandboxieInitialize(): Promise<SandboxieStatus> {
  const invoke = getInvoke();
  if (invoke) return invoke<SandboxieStatus>("sandboxie_initialize");
  return sandboxieStatus();
}

export async function sandboxieInstall(): Promise<SandboxieStatus> {
  const invoke = getInvoke();
  if (invoke) return invoke<SandboxieStatus>("sandboxie_install");
  return sandboxieStatus();
}

// ---- permission presets (Sandboxie integration) -----------------------------

/** Get the current active permission preset label. */
export async function getActivePermissionPreset(): Promise<PermissionPreset> {
  const invoke = getInvoke();
  if (invoke) return invoke<PermissionPreset>("get_active_permission_preset");
  return "default";
}

/** Set the active permission preset. */
export async function setActivePermissionPreset(preset: PermissionPreset): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("set_active_permission_preset", { preset });
}

/** Get which permission presets are visible in the Composer dropdown. */
export async function getPermissionPresetVisibility(): Promise<PermissionPresetVisibility> {
  const invoke = getInvoke();
  if (invoke) return invoke<PermissionPresetVisibility>("get_permission_preset_visibility");
  return { default_enabled: true, auto_review_enabled: true, full_access_enabled: true };
}

/** Set which permission presets are visible in the Composer dropdown. */
export async function setPermissionPresetVisibility(
  visibility: PermissionPresetVisibility,
): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("set_permission_preset_visibility", { visibility });
}

export interface PreflightToolCall {
  call_id: string;
  name: string;
  arguments: unknown;
  ok: boolean;
  output: unknown;
  duration_ms: number;
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
  runId?: string,
  envMode?: string | null,
  connectionId?: string | null,
  preflightTools: PreflightToolCall[] = [],
  preflightAbortMessage?: string | null,
  initialPlanMode = false
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
      const nextSessionId = await invoke<string>("run_chat", {
        prompt,
        sessionId: sessionId ?? null,
        envMode: envMode ?? null,
        connectionId: connectionId ?? null,
        runId: actualRunId,
        preflightTools,
        preflightAbortMessage: preflightAbortMessage ?? null,
        initialPlanMode,
      });
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

// ---- web-search provider settings ----------------------------------------

export async function getWebSearchSettings(): Promise<WebSearchSettings> {
  const invoke = getInvoke();
  if (invoke) return invoke<WebSearchSettings>("get_web_search_settings");
  return {
    enabled: true,
    provider: "deepseek_first",
    searxng_url: null,
    anysearch_enabled: false,
    anysearch_base_url: null,
    anysearch_api_key_configured: false,
  };
}

export async function setWebSearchSettings(
  settings: WebSearchSettings,
): Promise<WebSearchSettings> {
  const invoke = getInvoke();
  if (invoke) {
    const value = await invoke<WebSearchSettings>("set_web_search_settings", { settings });
    emitSettingsChanged();
    return value;
  }
  return settings;
}

/** Inspect whether an AnySearch API key is configured. */
export async function getAnySearchApiKeyInfo(): Promise<AnySearchApiKeyInfo> {
  const invoke = getInvoke();
  if (invoke) return invoke<AnySearchApiKeyInfo>("get_anysearch_api_key");
  return { has_user_key: false };
}

/** Save the AnySearch API key to the OS keychain. */
export async function setAnySearchApiKey(key: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("set_anysearch_api_key", { key });
}

/** Clear the AnySearch API key from the OS keychain. */
export async function clearAnySearchApiKey(): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("clear_anysearch_api_key");
}

/** Run a tiny search to validate the configured AnySearch key. */
export async function testAnySearchApiKey(): Promise<AnySearchTestResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<AnySearchTestResult>("test_anysearch_api_key");
  return { ok: false, error: "desktop runtime is unavailable", provider: null, count: null };
}

// ---- vision settings ------------------------------------------------------

export async function getVisionSettings(): Promise<VisionSettings> {
  const invoke = getInvoke();
  if (invoke) return invoke<VisionSettings>("get_vision_settings");
  return {
    mode: "system",
    provider: "modelscope",
    base_url: "https://api-inference.modelscope.cn/v1",
    api_key: null,
    api_key_configured: false,
    system_model: "moonshotai/Kimi-K2.5:DashScope",
    timeout_ms: 60000,
    auto_analyze_pasted_images: true,
    send_original_image_to_model: false,
  };
}

export async function setVisionSettings(settings: VisionSettings): Promise<VisionSettings> {
  const invoke = getInvoke();
  if (invoke) {
    const value = await invoke<VisionSettings>("set_vision_settings", { settings });
    emitSettingsChanged();
    return value;
  }
  return settings;
}

export async function visionRecognizeImage(
  request: VisionRecognizeRequest
): Promise<VisionRecognizeResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<VisionRecognizeResult>("vision_recognize_image", { request });
  throw new Error("vision recognition requires the desktop app");
}

export async function visionTestConnection(): Promise<VisionRecognizeResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<VisionRecognizeResult>("vision_test_connection");
  throw new Error("vision test requires the desktop app");
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

export async function testMcpServer(
  server: McpServer
): Promise<McpConnectionStatus> {
  const invoke = getInvoke();
  if (invoke) return invoke<McpConnectionStatus>("test_mcp_server", { server });
  return { name: server.name, status: "failed", error: "desktop app required", tools: [] };
}

export async function mcpConnectionStatus(): Promise<McpConnectionStatus[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<McpConnectionStatus[]>("mcp_connection_status");
  return mockMcpConnectionStatus();
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

// ---- git (read-only project/worktree state) -------------------------------

export async function gitProjectStatus(path: string): Promise<GitProjectStatus> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitProjectStatus>("git_project_status", { path });
  return mockGitProjectStatus(path);
}

export async function gitProjectsStatus(paths?: string[]): Promise<GitProjectStatus[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitProjectStatus[]>("git_projects_status", { paths: paths ?? null });
  return (paths ?? []).map(mockGitProjectStatus);
}

export async function gitBranchList(path: string): Promise<GitBranch[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitBranch[]>("git_branch_list", { path });
  return path
    ? [mockGitBranch("main", true), mockGitBranch("feature/git-workbench", false)]
    : [];
}

export async function gitCheckoutBranch(path: string, branch: string): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_checkout_branch", { path, branch });
  return { ok: true, command: `git switch ${branch}`, stdout: "mock checkout", stderr: "" };
}

export async function gitCreateBranch(
  path: string,
  name: string,
  startPoint?: string | null
): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<GitOperationResult>("git_create_branch", {
      path,
      name,
      startPoint: startPoint ?? null,
    });
  }
  return {
    ok: true,
    command: `git switch -c ${name}${startPoint ? ` ${startPoint}` : ""}`,
    stdout: "mock create branch",
    stderr: "",
  };
}

export async function openSessionInNewWindow(sessionId: string): Promise<void> {
  const url = `${window.location.origin}${window.location.pathname}${window.location.search}#session=${encodeURIComponent(sessionId)}`;
  if (typeof window !== "undefined" && (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const label = `session-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const webview = new WebviewWindow(label, {
      url,
      title: "DeepAgent Studio",
      width: 1280,
      height: 860,
      focus: true,
    });
    await new Promise<void>((resolve, reject) => {
      webview.once("tauri://created", () => resolve());
      webview.once("tauri://error", (event) => reject(new Error(String(event.payload))));
    });
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

export async function openStudioCanvasWindow(): Promise<void> {
  const url = `${window.location.origin}${window.location.pathname}${window.location.search}#window=canvas`;
  if (typeof window !== "undefined" && (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const label = "studio-canvas";
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) {
      await existing.unminimize().catch(() => {});
      await existing.show().catch(() => {});
      await existing.setFocus();
      return;
    }

    const webview = new WebviewWindow(label, {
      url,
      title: "DeepAgent Studio · 工作画布",
      width: 1280,
      height: 820,
      minWidth: 840,
      minHeight: 560,
      decorations: false,
      shadow: true,
      focus: true,
    });
    await new Promise<void>((resolve, reject) => {
      webview.once("tauri://created", () => resolve());
      webview.once("tauri://error", (event) => reject(new Error(String(event.payload))));
    });
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}

export async function gitChanges(path: string): Promise<GitChanges> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitChanges>("git_changes", { path });
  return {
    project_path: path,
    repo_root: path || null,
    is_repo: !!path,
    files: [],
    additions: 0,
    deletions: 0,
  };
}

export async function gitDiff(
  path: string,
  filePath: string,
  staged: boolean = false
): Promise<GitDiff> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitDiff>("git_diff", { path, filePath, staged });
  return {
    project_path: path,
    repo_root: path || null,
    file_path: filePath,
    staged,
    is_repo: !!path,
    text: "",
    truncated: false,
  };
}

export async function gitLog(path: string, limit: number = 200): Promise<GitLogEntry[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitLogEntry[]>("git_log", { path, limit });
  return [
    {
      hash: "mock001",
      full_hash: "mock001",
      parents: [],
      author_name: "DeepAgent",
      author_email: "preview@example.test",
      date: new Date().toISOString(),
      refs: ["HEAD -> main"],
      subject: "Browser preview commit",
      files: [],
    },
  ];
}

export async function gitCommitDiff(
  path: string,
  commit: string,
  filePath: string
): Promise<GitDiff> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitDiff>("git_commit_diff", { path, commit, filePath });
  return {
    project_path: path,
    repo_root: path || null,
    file_path: filePath,
    staged: false,
    is_repo: !!path,
    text: "",
    truncated: false,
  };
}

export async function gitStage(path: string, files: string[]): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_stage", { path, files });
  return { ok: true, command: "git add", stdout: "", stderr: "" };
}

export async function gitUnstage(path: string, files: string[]): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_unstage", { path, files });
  return { ok: true, command: "git restore --staged", stdout: "", stderr: "" };
}

export async function gitApplyHunk(
  path: string,
  filePath: string,
  patch: string,
  staged: boolean
): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_apply_hunk", { path, filePath, patch, staged });
  return {
    ok: true,
    command: staged ? "git apply --cached --reverse" : "git apply --cached",
    stdout: "mock hunk apply",
    stderr: "",
  };
}

export async function gitCommit(path: string, message: string, files?: string[]): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_commit", { path, message, files: files?.length ? files : null });
  return { ok: true, command: "git commit", stdout: "mock commit", stderr: "" };
}

export async function gitCommitMessageDraft(path: string): Promise<GitCommitMessageDraft> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitCommitMessageDraft>("git_commit_message_draft", { path });
  return mockGitCommitMessageDraft(path);
}

export async function gitPushPreview(path: string): Promise<GitPushPreview> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitPushPreview>("git_push_preview", { path });
  return mockGitPushPreview(path);
}

export async function gitPushRiskScan(path: string): Promise<GitPushRiskScan> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitPushRiskScan>("git_push_risk_scan", { path });
  return mockGitPushRiskScan(path);
}

export async function gitPush(
  path: string,
  remote?: string | null,
  branch?: string | null
): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_push", { path, remote: remote ?? null, branch: branch ?? null });
  return { ok: true, command: "git push origin HEAD:main", stdout: "mock push", stderr: "" };
}

export async function gitFetch(path: string, all: boolean = false): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_fetch", { path, all });
  return { ok: true, command: all ? "git fetch --all --prune" : "git fetch --prune", stdout: "mock fetch", stderr: "" };
}

export async function gitPullUpdate(path: string): Promise<GitOperationResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitOperationResult>("git_pull_update", { path });
  return { ok: true, command: "git pull --ff-only", stdout: "mock update", stderr: "" };
}

export async function gitWorktrees(path: string): Promise<GitWorktree[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitWorktree[]>("git_worktrees", { path });
  return [];
}

export async function gitCompareRefs(
  path: string,
  baseRef: string,
  targetRef: string
): Promise<GitRefCompare> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitRefCompare>("git_compare_refs", { path, baseRef, targetRef });
  return mockGitRefCompare(path, baseRef, targetRef);
}

export async function gitRefDiff(
  path: string,
  baseRef: string,
  targetRef: string,
  filePath?: string | null
): Promise<GitDiff> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<GitDiff>("git_ref_diff", {
      path,
      baseRef,
      targetRef,
      filePath: filePath ?? null,
    });
  }
  return {
    project_path: path,
    repo_root: path || null,
    file_path: filePath ?? "",
    staged: false,
    is_repo: !!path,
    text: `diff --git a/${filePath ?? "mock.txt"} b/${filePath ?? "mock.txt"}\n+mock ref diff\n`,
    truncated: false,
  };
}

export async function gitBatchCommitPreview(targets: string[]): Promise<GitBatchCommitPreviewItem[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitBatchCommitPreviewItem[]>("git_batch_commit_preview", { targets });
  return targets.map((projectPath) => ({
    project_path: projectPath,
    repo_root: projectPath || null,
    is_repo: !!projectPath,
    current_branch: "main",
    files_changed: 1,
    staged_files: 1,
    additions: 4,
    deletions: 1,
    ahead: 0,
    behind: 0,
    blocked_reason: projectPath ? null : "not a git repository",
  }));
}

export async function gitBatchCommit(
  targets: GitBatchCommitTarget[],
  message: string,
  stageAll: boolean
): Promise<GitBatchProjectResult[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitBatchProjectResult[]>("git_batch_commit", { targets, message, stageAll });
  return mockGitBatchResults(targets.map((target) => target.project_path), "Committed", true, false);
}

export async function gitBatchPush(targets: string[]): Promise<GitBatchProjectResult[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<GitBatchProjectResult[]>("git_batch_push", { targets });
  return mockGitBatchResults(targets, "Pushed", false, true);
}

export async function gitBatchCommitAndPush(
  targets: GitBatchCommitTarget[],
  message: string,
  stageAll: boolean
): Promise<GitBatchProjectResult[]> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<GitBatchProjectResult[]>("git_batch_commit_and_push", { targets, message, stageAll });
  }
  return mockGitBatchResults(targets.map((target) => target.project_path), "Committed and pushed", true, true);
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

export interface LocalPtyHandle {
  pty_id: string;
  cols: number;
  rows: number;
}

/** Open the user's system terminal in the active project directory. */
export async function openSystemTerminal(): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("open_system_terminal");
  throw new Error("opening the system terminal requires the desktop app");
}

export async function localPtySpawn(cols: number, rows: number): Promise<LocalPtyHandle> {
  const invoke = getInvoke();
  if (invoke) return invoke<LocalPtyHandle>("local_pty_spawn", { cols, rows });
  return { pty_id: "local-preview", cols, rows };
}

export async function localPtyWrite(ptyId: string, data: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("local_pty_write", { ptyId, data });
}

export async function localPtyRead(ptyId: string): Promise<number[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<number[]>("local_pty_read", { ptyId });
  return [];
}

export async function localPtyResize(ptyId: string, cols: number, rows: number): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("local_pty_resize", { ptyId, cols, rows });
}

export async function localPtyClose(ptyId: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("local_pty_close", { ptyId });
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

// ---- file preview (office-agent) ------------------------------------------

/**
 * Open the OS-native "open file" dialog filtered to previewable office/text
 * file types, returning the chosen absolute path or null if cancelled. Only
 * available inside the desktop app.
 */
export async function pickPreviewFile(): Promise<string | null> {
  if (!isTauri()) return null;
  const mod = await import("@tauri-apps/plugin-dialog");
  const selected = await mod.open({
    directory: false,
    multiple: false,
    title: "Select a file to preview",
    filters: [
      {
        name: "Office / Text / Image",
        extensions: [
          "docx", "xlsx", "pptx", "pdf",
          "txt", "md", "json", "csv", "tsv", "log", "yaml", "yml", "toml", "xml",
          "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg",
        ],
      },
    ],
  });
  if (typeof selected === "string") return selected;
  return null;
}

/**
 * Open the OS-native "open file" dialog for selecting an SSH identity file.
 * Returns the chosen absolute path or null if the user cancelled.
 */
export async function pickSshIdentityFile(): Promise<string | null> {
  if (!isTauri()) return null;
  const mod = await import("@tauri-apps/plugin-dialog");
  const selected = await mod.open({
    directory: false,
    multiple: false,
    title: "Select SSH Identity File",
  });
  if (typeof selected === "string") return selected;
  return null;
}

/** Read metadata (name / ext / size / classified kind) for a file. */
export async function previewGetMetadata(path: string): Promise<PreviewMetadata> {
  const invoke = getInvoke();
  if (invoke) return invoke<PreviewMetadata>("preview_get_metadata", { path });
  throw new Error("file preview requires the desktop app");
}

/** Open a file for preview: returns metadata + extracted content (text/sheets). */
export async function previewOpenFile(path: string): Promise<PreviewResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<PreviewResult>("preview_open_file", { path });
  throw new Error("file preview requires the desktop app");
}

/** Extract a previewable representation of a file (text/xlsx sheets). */
export async function previewExtractText(path: string): Promise<PreviewResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<PreviewResult>("preview_extract_text", { path });
  throw new Error("file preview requires the desktop app");
}

/** Read an image file as a base64 data URL for direct display in the webview. */
export async function previewReadDataUrl(path: string): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("preview_read_data_url", { path });
  throw new Error("file preview requires the desktop app");
}

/** Render PDF pages (Tier C degrades to text; pdfium adds page images). */
export async function previewRenderPages(path: string): Promise<PdfRenderResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<PdfRenderResult>("preview_render_pages", { path });
  throw new Error("file preview requires the desktop app");
}

export async function attachmentIngest(input: AttachmentIngestInput): Promise<PersistedAttachment> {
  const invoke = getInvoke();
  if (invoke) return invoke<PersistedAttachment>("attachment_ingest", { input });
  return {
    id: input.id ?? `preview-${Date.now()}`,
    session_id: input.session_id ?? null,
    kind: input.kind,
    name: input.name,
    mime: input.mime,
    size_bytes: input.text?.length ?? 0,
    source: input.source,
    storage_dir: "",
    original_path: input.local_path ?? null,
    extracted_text: input.text ?? null,
    preview: null,
    sha256: null,
    status: "ready",
    message: null,
  };
}

export async function attachmentRemove(id: string, sessionId?: string | null): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("attachment_remove", { id, sessionId: sessionId ?? null });
  return true;
}

/** List one directory level from the active/current project. */
export async function listProjectFiles(
  projectPath?: string | null,
  path?: string | null,
): Promise<ProjectFileListResult> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<ProjectFileListResult>("list_project_files", {
      projectPath: projectPath ?? null,
      path: path ?? null,
    });
  }
  return {
    root_path: projectPath ?? "",
    entries: [],
  };
}

/** Recursively fuzzy-search files and folders in the active/current project. */
export async function searchProjectFiles(
  projectPath?: string | null,
  query?: string | null,
  limit = 30,
): Promise<ProjectFileSearchResult> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<ProjectFileSearchResult>("search_project_files", {
      projectPath: projectPath ?? null,
      query: query ?? "",
      limit,
    });
  }
  return {
    root_path: projectPath ?? "",
    entries: [],
  };
}

// ---- recording (office-agent) ---------------------------------------------

export async function audioListInputDevices(): Promise<string[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<string[]>("audio_list_input_devices");
  return [];
}

export async function audioStartRecording(name: string): Promise<RecordingSession> {
  const invoke = getInvoke();
  if (invoke) return invoke<RecordingSession>("audio_start_recording", { name });
  throw new Error("recording requires the desktop app");
}

export async function audioPauseRecording(sessionId: string): Promise<RecordingSession> {
  const invoke = getInvoke();
  if (invoke) return invoke<RecordingSession>("audio_pause_recording", { sessionId });
  throw new Error("recording requires the desktop app");
}

export async function audioResumeRecording(sessionId: string): Promise<RecordingSession> {
  const invoke = getInvoke();
  if (invoke) return invoke<RecordingSession>("audio_resume_recording", { sessionId });
  throw new Error("recording requires the desktop app");
}

export async function audioStopRecording(sessionId: string): Promise<RecordingSession> {
  const invoke = getInvoke();
  if (invoke) return invoke<RecordingSession>("audio_stop_recording", { sessionId });
  throw new Error("recording requires the desktop app");
}

// ---- speech (office-agent) ------------------------------------------------

/** Transcribe a recorded WAV to timestamped segments. */
export async function speechTranscribeFile(wavPath: string): Promise<TranscriptSegment[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<TranscriptSegment[]>("speech_transcribe_file", { wavPath });
  throw new Error("transcription requires the desktop app");
}

/** Whether a speech model is installed (decide whether to prompt a download). */
export async function speechModelInstalled(): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("speech_model_installed");
  return false;
}

/** Whether the local whisper.cpp sidecar engine is installed. */
export async function speechEngineInstalled(): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("speech_engine_installed");
  return false;
}

/** Generate a structured Markdown meeting-minutes document from a transcript. */
export async function speechGenerateMeetingMinutes(transcript: string): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("speech_generate_meeting_minutes", { transcript });
  throw new Error("meeting minutes requires the desktop app");
}

// ---- office documents (office-agent) --------------------------------------

/** Read readable text from an office/text file (Tier C, pure Rust). */
export async function officeReadText(path: string): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("office_read_text", { path });
  throw new Error("office read requires the desktop app");
}

/** Create a .docx from Markdown at an explicit path; returns the path. */
export async function officeCreateDocxFromMarkdown(
  markdown: string,
  title: string | null,
  outPath: string
): Promise<string> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<string>("office_create_docx_from_markdown", { markdown, title, outPath });
  throw new Error("office docx generation requires the desktop app");
}

/** Export Markdown meeting-minutes to a .docx in the recordings folder. */
export async function officeExportMinutesDocx(markdown: string): Promise<string> {
  const invoke = getInvoke();
  if (invoke) return invoke<string>("office_export_minutes_docx", { markdown });
  throw new Error("minutes export requires the desktop app");
}

// ---- managed runtimes (office-agent) --------------------------------------

/** List all known managed runtimes and their install status. */
export async function runtimeList(): Promise<RuntimeStatus[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<RuntimeStatus[]>("runtime_list");
  return [];
}

export async function runtimeStatus(id: string): Promise<RuntimeStatus | null> {
  const invoke = getInvoke();
  if (invoke) return invoke<RuntimeStatus | null>("runtime_status", { id });
  return null;
}

export async function runtimeRoots(): Promise<RuntimeRoots> {
  const invoke = getInvoke();
  if (invoke) return invoke<RuntimeRoots>("runtime_roots");
  return { active_root: "", fallback_roots: [] };
}

export async function runtimeMigrateResources(): Promise<RuntimeStatus[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<RuntimeStatus[]>("runtime_migrate_resources");
  return [];
}

export async function runtimePrepareForUpdate(): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("runtime_prepare_for_update");
}

/**
 * Download + verify + install a managed runtime into the app data dir.
 * High-risk (downloads + writes a binary/model) — call only after explicit
 * user consent. Progress arrives via the `runtime:progress` event; subscribe
 * with {@link runtimeProgressSubscribe}.
 */
export async function runtimeInstall(id: string): Promise<RuntimeStatus> {
  const invoke = getInvoke();
  if (invoke) return invoke<RuntimeStatus>("runtime_install", { id });
  throw new Error("runtime install requires the desktop app");
}

export async function runtimeCancel(id: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) await invoke("runtime_cancel", { id });
}

export async function runtimeUninstall(id: string): Promise<boolean> {
  const invoke = getInvoke();
  if (invoke) return invoke<boolean>("runtime_uninstall", { id });
  return false;
}

/** Subscribe to `runtime:progress` events for a runtime id. Returns an
 *  unlisten function. */
export async function runtimeProgressSubscribe(
  id: string,
  onProgress: (p: RuntimeProgress) => void
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const mod = await import("@tauri-apps/api/event");
  const unlisten = await mod.listen<RuntimeProgress>("runtime:progress", (event) => {
    if (event.payload && event.payload.id === id) onProgress(event.payload);
  });
  return () => unlisten();
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

function mockMcpConnectionStatus(): McpConnectionStatus[] {
  return [
    {
      name: "node_repl",
      status: "connected",
      error: null,
      tools: [{ name: "eval", description: "Evaluate a JS expression" }],
    },
  ];
}

function mockGitProjectStatus(path: string): GitProjectStatus {
  return {
    project_path: path,
    repo_root: path || null,
    repo_id: path ? `mock:${path}` : null,
    is_repo: !!path,
    current_branch: path ? "main" : null,
    detached_head: false,
    upstream: path ? "origin/main" : null,
    ahead: 0,
    behind: 0,
    has_changes: false,
    files_changed: 0,
    additions: 0,
    deletions: 0,
    rebase_state: null,
    merge_state: false,
    gh_available: false,
  };
}

function mockGitPushPreview(path: string): GitPushPreview {
  return {
    project_path: path,
    repo_root: path || null,
    is_repo: !!path,
    current_branch: path ? "main" : null,
    upstream: path ? "origin/main" : null,
    remote: path ? "origin" : null,
    remote_branch: path ? "main" : null,
    ahead: 1,
    behind: 0,
    commits: [
      {
        hash: "mock001",
        full_hash: "mock001",
        author_name: "DeepAgent",
        date: new Date().toISOString(),
        subject: "Browser preview commit",
      },
    ],
    blocked_reason: path ? null : "not a git repository",
  };
}

function mockGitCommitMessageDraft(path: string): GitCommitMessageDraft {
  return {
    project_path: path,
    repo_root: path || null,
    is_repo: !!path,
    source: path ? "working_tree" : "none",
    title: path ? "chore: update Git Workbench changes" : "",
    body: path
      ? "- Update 2 file(s) (+42, -3)\n- M apps/desktop/src/components/git/GitChangesPanel.tsx (+28, -2)"
      : "",
    files: [],
    blocked_reason: path ? null : "not a git repository",
  };
}

function mockGitPushRiskScan(path: string): GitPushRiskScan {
  return {
    project_path: path,
    repo_root: path || null,
    is_repo: !!path,
    current_branch: path ? "main" : null,
    upstream: path ? "origin/main" : null,
    ahead: path ? 1 : 0,
    scanned_files: path ? 3 : 0,
    risks: path
      ? [
          {
            severity: "low",
            category: "debug_log",
            title: "Debug output added",
            detail: "console.log('debug')",
            file_path: "apps/desktop/src/components/git/GitPushPanel.tsx",
          },
        ]
      : [],
    blocked_reason: path ? null : "not a git repository",
  };
}

function mockGitRefCompare(path: string, baseRef: string, targetRef: string): GitRefCompare {
  return {
    project_path: path,
    repo_root: path || null,
    is_repo: !!path,
    base_ref: baseRef || "main",
    target_ref: targetRef || "feature/git-workbench",
    merge_base: path ? "mock-base" : null,
    ahead: path ? 2 : 0,
    behind: path ? 1 : 0,
    commits: path
      ? [
          {
            side: "target",
            hash: "mock301",
            full_hash: "mock301",
            author_name: "DeepAgent",
            date: new Date().toISOString(),
            subject: "Add branch comparison panel",
          },
          {
            side: "base",
            hash: "mock201",
            full_hash: "mock201",
            author_name: "DeepAgent",
            date: new Date().toISOString(),
            subject: "Upstream fix not in target",
          },
        ]
      : [],
    files: path
      ? [
          {
            path: "apps/desktop/src/components/git/GitProjectsPanel.tsx",
            old_path: null,
            status: "M",
            additions: 36,
            deletions: 4,
          },
        ]
      : [],
    blocked_reason: path ? null : "not a git repository",
  };
}

function mockGitBatchResults(
  targets: string[],
  message: string,
  committed: boolean,
  pushed: boolean
): GitBatchProjectResult[] {
  return targets.map((projectPath) => ({
    project_path: projectPath,
    current_branch: "main",
    ok: true,
    committed,
    pushed,
    skipped: false,
    message,
    commit_result: committed
      ? { ok: true, command: "git commit", stdout: "mock commit", stderr: "" }
      : null,
    push_result: pushed
      ? { ok: true, command: "git push", stdout: "mock push", stderr: "" }
      : null,
  }));
}

function mockGitBranch(name: string, current: boolean): GitBranch {
  return {
    name,
    full_name: `refs/heads/${name}`,
    kind: "local",
    current,
    upstream: name === "main" ? "origin/main" : null,
    ahead: current ? 0 : 1,
    behind: 0,
    commit: "mock",
    subject: current ? "Current browser-preview branch" : "Preview branch",
    worktree_path: null,
  };
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

// ==================== SSH Long-Lived Connection API ====================

export interface SshConnection {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth_type: "agent" | "key_file" | "password";
  key_path?: string;
  status: "disconnected" | "connecting" | "connected" | "error";
  last_error?: string;
  latency_ms?: number;
}

export interface SshExecResult {
  exit_code: number;
  stdout: string;
  stderr: string;
  duration_ms: number;
}

export interface SshTestResult {
  ok: boolean;
  latency_ms?: number;
  banner?: string;
  error?: string;
}

export interface RemoteProbeResult {
  os?: string;
  distro?: string;
  distro_version?: string;
  arch?: string;
  shell?: string;
  user?: string;
  cwd?: string;
  path?: string;
  package_managers: string[];
  commands: Record<string, boolean>;
  runtimes: Record<string, string>;
  probed_at_ms: number;
}

export interface RemotePushFileRequest {
  local_path: string;
  remote_path: string;
  create_parent?: boolean;
  overwrite?: boolean;
  verify_mode?: "none" | "size" | "sha256";
}

export interface RemotePushFileResult {
  ok: boolean;
  remote_path: string;
  bytes: number;
  local_sha256?: string;
  remote_sha256?: string;
  integrity_verified: boolean;
  duration_ms: number;
}

export interface RemoteBundleRequest {
  local_path: string;
  remote_path: string;
  create_parent?: boolean;
  overwrite?: boolean;
  verify_mode?: "none" | "size" | "sha256";
  remove_archive_after_extract?: boolean;
}

export interface RemoteBundleResult {
  ok: boolean;
  remote_path: string;
  remote_archive_path: string;
  remote_manifest_path: string;
  files: number;
  bytes: number;
  local_archive_sha256?: string;
  remote_archive_sha256?: string;
  integrity_verified: boolean;
  extract_verified: boolean;
  duration_ms: number;
}

export interface RemoteRuntimeRequirement {
  name: string;
  version?: string;
}

export interface RemoteRequireRequest {
  commands?: string[];
  runtimes?: RemoteRuntimeRequirement[];
  archives?: string[];
}

export interface RemoteRequireResult {
  package_manager?: string;
  package_managers: string[];
  missing_commands: string[];
  missing_runtimes: string[];
  missing_archive_tools: string[];
  install_commands: string[];
  can_install: boolean;
  probe: RemoteProbeResult;
}

export interface RemoteInstallRequest {
  package_manager?: string;
  commands?: string[];
  runtimes?: RemoteRuntimeRequirement[];
  packages?: string[];
  update_index?: boolean;
}

export interface RemoteInstallResult {
  ok: boolean;
  package_manager?: string;
  commands_run: string[];
  stdout: string;
  stderr: string;
  installed_packages: string[];
  probe?: RemoteProbeResult;
}

export async function sshListConnections(): Promise<SshConnection[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<SshConnection[]>("ssh_list_connections");
  return [];
}

export async function sshCreateConnection(
  name: string,
  host: string,
  port: number,
  username: string,
  authType: "agent" | "key_file" | "password",
  keyPath?: string,
  password?: string,
): Promise<SshConnection> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<SshConnection>("ssh_create_connection", {
      name,
      host,
      port,
      username,
      authType,
      keyPath,
      password,
    });
  return { id: "mock", name, host, port, username, auth_type: authType, status: "disconnected" };
}

export async function sshUpdateConnection(
  id: string,
  name: string,
  host: string,
  port: number,
  username: string,
  authType: "agent" | "key_file" | "password",
  keyPath?: string,
  password?: string,
): Promise<SshConnection> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<SshConnection>("ssh_update_connection", {
      id,
      name,
      host,
      port,
      username,
      authType,
      keyPath,
      password,
    });
  return { id, name, host, port, username, auth_type: authType, status: "disconnected" };
}

export async function sshRemoveConnection(id: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("ssh_remove_connection", { id });
}

export async function sshConnect(id: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("ssh_connect", { id });
}

export async function sshDisconnect(id: string): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("ssh_disconnect", { id });
}

export async function sshStatus(id: string): Promise<SshConnection> {
  const invoke = getInvoke();
  if (invoke) return invoke<SshConnection>("ssh_status", { id });
  return { id: id, name: "", host: "", port: 22, username: "", auth_type: "agent", status: "disconnected" };
}

export async function sshTestConnection(
  id: string,
): Promise<SshTestResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<SshTestResult>("ssh_test_connection", { id });
  return { ok: false, error: "not available in browser preview" };
}

export async function sshExec(
  connectionId: string,
  command: string,
): Promise<SshExecResult> {
  const invoke = getInvoke();
  if (invoke)
    return invoke<SshExecResult>("ssh_exec", { connectionId, command });
  return {
    exit_code: 0,
    stdout: "[browser preview] ssh commands require the desktop app",
    stderr: "",
    duration_ms: 0,
  };
}

export async function sshRemoteProbe(
  connectionId: string,
  forceRefresh = false,
): Promise<RemoteProbeResult> {
  const invoke = getInvoke();
  if (invoke) {
    return invoke<RemoteProbeResult>("ssh_remote_probe", { connectionId, forceRefresh });
  }
  return {
    package_managers: [],
    commands: {},
    runtimes: {},
    probed_at_ms: Date.now(),
  };
}

export async function sshPushFile(
  connectionId: string,
  request: RemotePushFileRequest,
): Promise<RemotePushFileResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<RemotePushFileResult>("ssh_push_file", { connectionId, request });
  return {
    ok: false,
    remote_path: request.remote_path,
    bytes: 0,
    integrity_verified: false,
    duration_ms: 0,
  };
}

export async function sshPushBundle(
  connectionId: string,
  request: RemoteBundleRequest,
): Promise<RemoteBundleResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<RemoteBundleResult>("ssh_push_bundle", { connectionId, request });
  return {
    ok: false,
    remote_path: request.remote_path,
    remote_archive_path: "",
    remote_manifest_path: "",
    files: 0,
    bytes: 0,
    integrity_verified: false,
    extract_verified: false,
    duration_ms: 0,
  };
}

export async function sshRemoteRequire(
  connectionId: string,
  request: RemoteRequireRequest,
): Promise<RemoteRequireResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<RemoteRequireResult>("ssh_remote_require", { connectionId, request });
  return {
    package_managers: [],
    missing_commands: [],
    missing_runtimes: [],
    missing_archive_tools: [],
    install_commands: [],
    can_install: false,
    probe: {
      package_managers: [],
      commands: {},
      runtimes: {},
      probed_at_ms: Date.now(),
    },
  };
}

export async function sshRemoteInstall(
  connectionId: string,
  request: RemoteInstallRequest,
): Promise<RemoteInstallResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<RemoteInstallResult>("ssh_remote_install", { connectionId, request });
  return {
    ok: false,
    commands_run: [],
    stdout: "",
    stderr: "not available in browser preview",
    installed_packages: [],
  };
}

// ---- SSH PTY streaming (interactive terminal) ----------------------------

export interface SshPtyHandle {
  connection_id: string;
  token: string;
  cols: number;
  rows: number;
}

export async function sshPtySpawn(
  connectionId: string,
  cols: number,
  rows: number,
): Promise<SshPtyHandle> {
  const invoke = getInvoke();
  if (invoke) return invoke<SshPtyHandle>("ssh_pty_spawn", { connectionId, cols, rows });
  return { connection_id: connectionId, token: "", cols, rows };
}

export async function sshPtyWrite(
  connectionId: string,
  data: string,
): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("ssh_pty_write", { connectionId, data });
}

export async function sshPtyRead(connectionId: string): Promise<number[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<number[]>("ssh_pty_read", { connectionId });
  return [];
}

export async function sshPtyResize(
  connectionId: string,
  cols: number,
  rows: number,
): Promise<void> {
  const invoke = getInvoke();
  if (invoke) return invoke<void>("ssh_pty_resize", { connectionId, cols, rows });
}

