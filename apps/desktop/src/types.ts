// DTOs mirroring deepagent-app-core::dto (the kernel↔UI contract).

export interface SessionSummary {
  id: string;
  project?: string;
  title: string | null;
  mode: string;
  created_at: number;
  updated_at: number;
  ended: boolean;
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
  category: string;
  shortcut: string | null;
}

export interface ApprovalRequest {
  call_id: string;
  tool: string;
  risk: string;
  arguments: string;
  reason: string;
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

/** A message rendered in the chat view. */
export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
  tone?: "normal" | "error";
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
}

/** A project folder in the sidebar (mirrors deepagent-app-core::ProjectDto). */
export interface Project {
  name: string;
  path: string;
  session_count: number;
  updated_at: number;
}
