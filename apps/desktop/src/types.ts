// DTOs mirroring deepagent-app-core::dto (the kernel↔UI contract).

export interface SessionSummary {
  id: string;
  project?: string;
  title: string | null;
  mode: string;
  created_at: number;
  updated_at: number;
  ended: boolean;
  pinned: boolean;
}

export interface TimelineEntry {
  sequence: number;
  timestamp: number;
  kind: string;
  icon: string;
  label: string;
  detail: string | null;
  duration_ms: number | null;
}

export interface SessionStats {
  event_count: number;
  messages: number;
  tool_calls: number;
  tool_successes: number;
  tool_failures: number;
  total_tool_ms: number;
  tool_success_rate: number | null;
  duration_ms: number;
}

export interface SessionDetail {
  summary: SessionSummary;
  timeline: TimelineEntry[];
  stats: SessionStats;
}

export interface Command {
  id: string;
  title: string;
  description: string;
  category: string;
  shortcut: string | null;
}

export interface ApprovalRequest {
  call_id: string;
  tool: string;
  risk: string;
  arguments: string;
  reason: string;
  run_id?: string;
}

export type DiffKind = "context" | "added" | "removed";

export interface DiffLine {
  kind: DiffKind;
  old_line: number | null;
  new_line: number | null;
  content: string;
}

export interface DiffResult {
  lines: DiffLine[];
  added: number;
  removed: number;
}

/** A live tool-call card shown inline in an assistant message. */
export interface ToolCall {
  /** Correlation id from the runtime (matches started→completed events). */
  call_id: string;
  /** Tool name, e.g. "web_search". */
  name: string;
  /** JSON-stringified arguments (rendered, streamed as they arrive). */
  args: string;
  /** Lifecycle status of this call. */
  status: "running" | "ok" | "error" | "blocked";
  /** Wall-clock duration in ms once completed. */
  durationMs?: number;
  /** Short result/error summary once completed. */
  detail?: string;
}

/** One ordered segment of an assistant turn, in the order it streamed in. */
export type MessagePart =
  | { kind: "reasoning"; text: string }
  | { kind: "text"; text: string; tone?: "normal" | "error" }
  | { kind: "tool"; tool: ToolCall };

/** A reconstructed conversation part from a replayed session (mirrors
 * deepagent-app-core::ConversationPartDto). */
export type ConversationPart =
  | { kind: "text"; text: string }
  | { kind: "reasoning"; text: string }
  | {
      kind: "tool";
      call_id: string;
      name: string;
      args: string;
      status: string;
      duration_ms: number | null;
      detail: string | null;
    };

/** A reconstructed conversation message (mirrors ConversationMessageDto). */
export interface ConversationMessage {
  role: "user" | "assistant";
  content: string;
  parts: ConversationPart[];
  usage?: ConversationUsage;
}

/** Persisted per-turn usage (mirrors ConversationUsageDto). */
export interface ConversationUsage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  prompt_cache_hit_tokens: number;
  prompt_cache_miss_tokens: number;
  duration_ms: number;
}

/** Accumulated token usage for an assistant turn (mirrors runtime Usage). */
export interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cacheHitTokens: number;
  cacheMissTokens: number;
}

/** Budget configuration (mirrors deepagent-app-core::BudgetConfig). */
export interface BudgetConfig {
  daily_limit: number | null;
  monthly_limit: number | null;
}

/** Accumulated cost summary (mirrors deepagent-app-core::CostSummary). */
export interface CostSummary {
  session_cost: number;
  today_cost: number;
  month_cost: number;
  total_cost: number;
  currency: string;
  budget: BudgetConfig;
}

/** One per-currency balance row (mirrors deepagent-app-core::BalanceInfoDto). */
export interface BalanceInfo {
  currency: string;
  total_balance: string;
  granted_balance: string;
  topped_up_balance: string;
}

/** Account balance summary (mirrors deepagent-app-core::BalanceDto). */
export interface Balance {
  is_available: boolean;
  infos: BalanceInfo[];
}

export type DiagStatus = "ok" | "warning" | "error";

export interface DiagnosticResult {
  name: string;
  status: DiagStatus;
  detail: string;
  fix_hint: string | null;
}

/** A message rendered in the chat view. */
export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
  tone?: "normal" | "error";
  /** Tool-call cards attached to an assistant turn (in invocation order). */
  tools?: ToolCall[];
  /** DeepSeek Thinking-Mode reasoning trace (shown as a collapsible block). */
  reasoning?: string;
  /**
   * Ordered parts of a live assistant turn (reasoning / tool / text) in the
   * exact order they streamed. When present, the view renders these in
   * sequence (preserving chronology); otherwise it falls back to the legacy
   * tools/reasoning/content fields (used by replayed timeline messages).
   */
  parts?: MessagePart[];
  /** Accumulated token usage for this turn (shown under the final answer). */
  usage?: TokenUsage;
  /** Wall-clock duration of the run that produced this turn, in ms. */
  runMs?: number;
}

/** A skill's Level-1 metadata (mirrors deepagent-app-core::SkillDto). */
export interface Skill {
  id: string;
  name: string;
  description: string;
  version: string | null;
  origin: string; // "workspace" | "user" | "installed" | "built_in"
  triggers: string[];
}

/** A disclosed skill body (mirrors deepagent-app-core::SkillActivationDto). */
export interface SkillActivation {
  id: string;
  body: string;
}

/** An MCP server entry (mirrors deepagent-app-core::McpServerDto). */
export interface McpServer {
  name: string;
  transport: string; // "stdio" | "sse" | "http" | "ws"
  enabled: boolean;
  command: string | null;
  args: string[];
  env: Record<string, string>;
  url: string | null;
  headers: Record<string, string>;
}

/** Declarative permission rules (mirrors deepagent-hooks::PermissionRules). */
export interface PermissionRules {
  allow: string[];
  ask: string[];
  deny: string[];
}

/** Result of forking a session (mirrors deepagent-app-core::ForkResultDto). */
export interface ForkResult {
  new_session_id: string;
  source_session_id: string;
  forked_at: number;
}

/** Result of rewinding a session (mirrors deepagent-app-core::RewindResultDto). */
export interface RewindResult {
  session_id: string;
  kept_through: number;
  events_removed: number;
}

/** An exported session transcript (mirrors deepagent-app-core::TranscriptDto). */
export interface Transcript {
  session_id: string;
  format: string;
  extension: string;
  content: string;
}

/** The active project folder (mirrors deepagent-app-core::WorkspaceInfoDto). */
export interface WorkspaceInfo {
  name: string;
  path: string;
}

/** Redacted settings view (mirrors deepagent-app-core::SettingsView). */
export interface SettingsView {
  api_key_masked: string;
  base_url: string;
  available_models: string[];
  chat_model: string;
  reasoner_model: string;
  configured: boolean;
  approval_policy: string;
  sandbox_mode: "read_only" | "workspace_write" | "full_access";
  thinking_depth: "simple" | "medium" | "deep";
}

/** A project folder in the sidebar (mirrors deepagent-app-core::ProjectDto). */
export interface Project {
  name: string;
  path: string;
  pinned: boolean;
  session_count: number;
  updated_at: number;
}

export interface ArchivedConversation {
  session_id: string;
  title: string | null;
  project: string | null;
  project_path: string | null;
  archived_at: number;
  updated_at: number;
}

export interface ArchiveProjectResult {
  project_path: string;
  project_name: string;
  archived_count: number;
}

export interface ProjectMapStatus {
  status: "missing" | "ready" | "stale" | "updating" | "failed" | string;
  source: string | null;
  graph_path: string | null;
  updated_at: number | null;
  nodes: number;
  edges: number;
  files: number;
  functions: number;
  classes: number;
  last_error: string | null;
}

export interface ProjectMapHit {
  node_id: string;
  node_type: string;
  name: string;
  file_path: string | null;
  summary: string;
  complexity: "simple" | "moderate" | "complex" | string;
  score: number;
}

export interface ProjectMapEdge {
  source: string;
  target: string;
  edge_type: string;
  weight: number;
}

export interface ProjectMapGraph {
  nodes: ProjectMapHit[];
  edges: ProjectMapEdge[];
}

export interface ProjectMapOverview {
  status: ProjectMapStatus;
  project_name: string | null;
  description: string | null;
  languages: string[];
  frameworks: string[];
  complex_nodes: ProjectMapHit[];
}

export interface ProjectMapNode {
  id: string;
  node_type: string;
  name: string;
  file_path: string | null;
  line_range: [number, number] | null;
  summary: string;
  tags: string[];
  complexity: string;
  language_notes: string | null;
}

export interface ProjectMapNeighbor {
  edge_type: string;
  direction: string;
  node: ProjectMapHit;
}

export interface ProjectMapNeighbors {
  node: ProjectMapNode | null;
  imports: ProjectMapNeighbor[];
  imported_by: ProjectMapNeighbor[];
  calls: ProjectMapNeighbor[];
  called_by: ProjectMapNeighbor[];
  related: ProjectMapNeighbor[];
}

export interface ProjectMapImpact {
  target: ProjectMapNode | null;
  direct: ProjectMapHit[];
  indirect: ProjectMapHit[];
}

export interface ProjectMapRefresh {
  ok: boolean;
  graph_path: string;
  files: number;
  nodes: number;
  edges: number;
  duration_ms: number;
  truncated: boolean;
  message: string;
  status: ProjectMapStatus;
}

/** A knowledge entry (mirrors deepagent-app-core::KnowledgeDto). */
export interface KnowledgeEntry {
  id: string;
  title: string;
  kind: string; // "pitfall" | "solution" | "command" | "config" | "note"
  tags: string[];
  scope: string; // "project" | "global"
  created_at: number;
  updated_at: number;
  source_session: string | null;
  body: string;
}

/** A knowledge search hit (mirrors deepagent-app-core::KnowledgeHitDto). */
export interface KnowledgeHit {
  id: string;
  title: string;
  kind: string;
  scope: string;
  score: number;
  excerpt: string;
}

/** A draft to create/update a knowledge entry (mirrors KnowledgeDraftDto). */
export interface KnowledgeDraft {
  title: string;
  body: string;
  kind?: string | null;
  tags: string[];
  scope?: string | null;
  source_session?: string | null;
}
