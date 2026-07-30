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

export interface SessionUiPrefs {
  env_panel_auto_open: boolean;
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

export interface RuntimeLogEntry {
  id: number;
  ts_ms: number;
  level: string;
  category: string;
  event: string;
  run_id: string | null;
  session_id: string | null;
  task_id: string | null;
  correlation_id: string | null;
  source: string | null;
  message: string | null;
  data: unknown;
}

export interface Command {
  id: string;
  title: string;
  description: string;
  category: string;
  shortcut: string | null;
}

export interface ComposerSkillSelection {
  id: string;
  name: string;
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
  /** Raw tool output, when available, for richer tool cards. */
  output?: unknown;
  /** Coarse backend-provided category for row-based rendering. */
  toolKind?: string;
  /** Primary file path / URL / target, when known. */
  filePath?: string;
  /** Compact backend-provided one-line summary. */
  summary?: string;
  /** Lightweight structured backend hints for the row renderer. */
  meta?: Record<string, unknown>;
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
      output?: unknown | null;
      tool_kind?: string | null;
      file_path?: string | null;
      summary?: string | null;
      meta?: Record<string, unknown> | null;
    };

/** A reconstructed conversation message (mirrors ConversationMessageDto). */
export interface ConversationMessage {
  role: "user" | "assistant";
  content: string;
  attachments?: PersistedAttachment[];
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
  cost_yuan?: number;
}

/** Accumulated token usage for an assistant turn (mirrors runtime Usage). */
export interface TokenUsage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  cacheHitTokens: number;
  cacheMissTokens: number;
  costYuan?: number;
}

export type ContextBlockKind =
  | "stable_prefix"
  | "dynamic_runtime"
  | "task_summary"
  | "recent_conversation"
  | "retrieved_knowledge"
  | "retrieved_code"
  | "tool_schemas"
  | "tool_result_refs"
  | "user_goal"
  | "other";

export type ContextCacheScope = "stable_prefix" | "dynamic";

export interface ContextBlockUsage {
  name: string;
  kind: ContextBlockKind;
  tokens: number;
  cache_scope: ContextCacheScope;
  source: string;
}

export interface ContextUsageSnapshot {
  model_id: string;
  context_window: number;
  prompt_budget: number;
  estimated_prompt_tokens: number;
  used_ratio: number;
  reserved_output_tokens: number;
  reserved_tool_tokens: number;
  cache_hit_tokens: number;
  cache_miss_tokens: number;
  cache_hit_ratio?: number;
  compacted: boolean;
  blocks: ContextBlockUsage[];
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
  /** Inline skill chips selected from slash/typeahead for this user turn. */
  selectedSkills?: ComposerSkillSelection[];
  /** Inline @ mentions selected as context for this user turn. */
  mentions?: ComposerMention[];
  /** Attachments displayed with a user turn. Model-readable attachment text is
   * kept out of this field and injected only into the backend prompt. */
  attachments?: ComposerAttachment[];
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

export type ComposerAttachmentKind = "text" | "image" | "file";
export type ComposerAttachmentSource = "paste" | "drop" | "picker";
export type ComposerAttachmentStatus = "ready" | "processing" | "error";

export interface ComposerAttachment {
  id: string;
  kind: ComposerAttachmentKind;
  name: string;
  mime: string;
  size: number;
  source: ComposerAttachmentSource;
  localPath?: string;
  dataUrl?: string;
  extractedText?: string;
  storageDir?: string;
  originalPath?: string;
  sha256?: string;
  backendMessage?: string;
  status: ComposerAttachmentStatus;
  error?: string;
}

export interface AttachmentIngestInput {
  id?: string | null;
  session_id?: string | null;
  kind: ComposerAttachmentKind;
  name: string;
  mime: string;
  source: ComposerAttachmentSource;
  local_path?: string | null;
  data_url?: string | null;
  text?: string | null;
}

export interface PersistedAttachment {
  id: string;
  session_id?: string | null;
  kind: ComposerAttachmentKind;
  name: string;
  mime: string;
  size_bytes: number;
  source: ComposerAttachmentSource;
  storage_dir: string;
  original_path?: string | null;
  extracted_text?: string | null;
  preview?: PreviewResult | null;
  sha256?: string | null;
  status: "ready" | "error";
  message?: string | null;
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
  source: "user" | "plugin" | string;
  source_plugin_id?: string | null;
  source_plugin_name?: string | null;
  declared_name?: string | null;
  source_path?: string | null;
  read_only: boolean;
  conflict?: string | null;
}

/** A tool exposed by a connected MCP server (mirrors McpToolInfoDto). */
export interface McpToolInfo {
  name: string;
  description: string;
}

/** Live connection status of an MCP server (mirrors McpConnectionStatusDto). */
export interface McpConnectionStatus {
  name: string;
  status: "connected" | "failed" | "disabled";
  error: string | null;
  tools: McpToolInfo[];
}

export interface PluginLoadError {
  kind: string;
  plugin?: string | null;
  source: string;
  path?: string | null;
  component?: string | null;
  message: string;
}

export interface PluginSourceInfo {
  kind: string;
  name: string;
  marketplace?: string | null;
  path?: string | null;
}

export interface PluginDependent {
  id: string;
  name: string;
  display_name: string;
}

export interface Plugin {
  id: string;
  name: string;
  display_name: string;
  description: string;
  long_description?: string | null;
  version?: string | null;
  local_version?: string | null;
  developer?: string | null;
  source: PluginSourceInfo;
  origin: string;
  path?: string | null;
  manifest_path?: string | null;
  installed: boolean;
  enabled: boolean;
  available: boolean;
  update_available: boolean;
  overridden_by?: string | null;
  category?: string | null;
  keywords: string[];
  capabilities: string[];
  permissions: string[];
  skill_count: number;
  mcp_server_count: number;
  hook_count: number;
  command_count: number;
  app_count: number;
  output_style_count: number;
  icon_path?: string | null;
  logo_path?: string | null;
  brand_color?: string | null;
  required_by: PluginDependent[];
  errors: PluginLoadError[];
}

export interface PluginApp {
  plugin_id: string;
  plugin_name: string;
  id: string;
  title: string;
  description?: string | null;
  placement: string;
  component: string;
  icon?: string | null;
  category?: string | null;
  source_path?: string | null;
}

export interface PluginOutputStyle {
  plugin_id: string;
  plugin_name: string;
  name: string;
  description: string;
  prompt: string;
  force_for_plugin?: boolean | null;
  source_path?: string | null;
}

export interface CreatePluginDraft {
  name: string;
  description?: string | null;
  directory?: string | null;
  category?: string | null;
}

export interface PluginMarketplace {
  name: string;
  source: string;
  git_ref?: string | null;
  sparse_path?: string | null;
  install_location?: string | null;
  last_updated?: string | null;
}

export interface PluginMarketplaceEntry {
  marketplace: string;
  name: string;
  display_name: string;
  description: string;
  version?: string | null;
  category?: string | null;
  source_kind: string;
  source: string;
  installable: boolean;
  install_block_reason?: string | null;
  installed: boolean;
  enabled: boolean;
  update_available: boolean;
  policy_installation?: string | null;
  policy_authentication?: string | null;
  authentication_required: boolean;
  authentication_hint?: string | null;
}

export interface AddPluginMarketplaceInput {
  name?: string | null;
  source: string;
  git_ref?: string | null;
  sparse_path?: string | null;
}

export interface PluginRiskItem {
  severity: string;
  category: string;
  title: string;
  detail: string;
  path?: string | null;
}

export interface PluginComponentSummary {
  kind: string;
  name: string;
  description: string;
  path?: string | null;
  details: string[];
}

export interface PluginScanReport {
  source_dir: string;
  manifest_ok: boolean;
  plugin_name?: string | null;
  file_count: number;
  total_bytes: number;
  component_summaries: PluginComponentSummary[];
  risks: PluginRiskItem[];
  errors: string[];
}

/** One row in a structured slash-command panel (mirrors SlashPanelItem). */
export interface SlashPanelItem {
  label: string;
  value?: string | null;
  status?: "ok" | "warn" | "error" | "muted" | "info" | null;
  badges?: string[];
  mono?: boolean;
  children?: SlashPanelItem[];
}

/** A group of rows under an optional heading (mirrors SlashSection). */
export interface SlashSection {
  heading?: string | null;
  items: SlashPanelItem[];
}

/** Structured info panel for slash commands (mirrors SlashPanel). */
export interface SlashPanel {
  title: string;
  subtitle?: string | null;
  sections: SlashSection[];
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
  restored_paths: string[];
}

/** Result of rewinding a session (mirrors deepagent-app-core::RewindResultDto). */
export interface RewindResult {
  session_id: string;
  kept_through: number;
  events_removed: number;
  restored_paths: string[];
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

export interface SandboxieStatus {
  supported: boolean;
  ready: boolean;
  box_name: string;
  install_dir?: string | null;
  start_exe?: string | null;
  sbie_ini_exe?: string | null;
  bundled_installer?: string | null;
  message: string;
}

export type PermissionPreset = "default" | "auto_review" | "full_access";

export interface PermissionPresetVisibility {
  default_enabled: boolean;
  auto_review_enabled: boolean;
  full_access_enabled: boolean;
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
  terminal_shell: "powershell" | "command_prompt" | "git_bash" | "wsl";
  thinking_depth: "simple" | "medium" | "deep";
  web_search: WebSearchSettings;
  vision: VisionSettings;
  execution_features: ExecutionFeatures;
  /** Built-in output style (§7.1): default | explanatory | learning. */
  output_style: string;
}

/**
 * Opt-in advanced execution safeguards (§2.2/§2.3/§6.1/§6.2). All default OFF.
 * The matching `DEEPAGENT_*` env var force-enables each one regardless.
 */
export interface ExecutionFeatures {
  /** §2.3 stall/laziness detector: audit final answers for false completion. */
  stall_detector: boolean;
  /** §6.1 LLM command-injection guard on suspicious shell commands. */
  command_guard: boolean;
  /** §6.2 per-project trust gate: untrusted dirs escalate bash to approval. */
  project_trust: boolean;
  /** §2.2 adversarial goal verifier: skeptic panel audits goal coverage. */
  adversarial_verify: boolean;
}

export type WebSearchProvider = "deepseek_first" | "searxng" | "duckduckgo";

export interface WebSearchSettings {
  enabled: boolean;
  provider: WebSearchProvider;
  searxng_url: string | null;
  anysearch_enabled: boolean;
  anysearch_base_url: string | null;
  anysearch_api_key_configured: boolean;
}

export interface AnySearchApiKeyInfo {
  has_user_key: boolean;
}

export interface AnySearchTestResult {
  ok: boolean;
  error: string | null;
  provider: string | null;
  count: number | null;
}

export type VisionMode = "off" | "system" | "model";

export interface VisionSettings {
  mode: VisionMode;
  provider: string;
  base_url: string;
  api_key?: string | null;
  api_key_configured: boolean;
  system_model: string;
  timeout_ms: number;
  auto_analyze_pasted_images: boolean;
  send_original_image_to_model: boolean;
}

export interface VisionRecognizeRequest {
  image_path: string;
  prompt?: string | null;
}

export interface VisionRecognizeResult {
  model_id: string;
  model_path: string;
  text: string;
  raw_json: string;
}

/** A project folder in the sidebar (mirrors deepagent-app-core::ProjectDto). */
export interface Project {
  name: string;
  path: string;
  pinned: boolean;
  session_count: number;
  updated_at: number;
}

export interface GitProjectStatus {
  project_path: string;
  repo_root: string | null;
  repo_id: string | null;
  is_repo: boolean;
  current_branch: string | null;
  detached_head: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  has_changes: boolean;
  files_changed: number;
  additions: number;
  deletions: number;
  rebase_state: string | null;
  merge_state: boolean;
  gh_available: boolean;
}

export interface GitBranch {
  name: string;
  full_name: string;
  kind: "local" | "remote" | string;
  current: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  commit: string | null;
  subject: string | null;
  worktree_path: string | null;
}

export interface GitChangedFile {
  path: string;
  old_path: string | null;
  status: string;
  category: "staged" | "unstaged" | "untracked" | "conflicted" | string;
  additions: number;
  deletions: number;
}

export interface GitChanges {
  project_path: string;
  repo_root: string | null;
  is_repo: boolean;
  files: GitChangedFile[];
  additions: number;
  deletions: number;
}

export interface GitDiff {
  project_path: string;
  repo_root: string | null;
  file_path: string;
  staged: boolean;
  is_repo: boolean;
  text: string;
  truncated: boolean;
}

export interface GitCommitFile {
  path: string;
  old_path: string | null;
  status: string;
  additions: number;
  deletions: number;
}

export interface GitLogEntry {
  hash: string;
  full_hash: string;
  parents: string[];
  author_name: string;
  author_email: string;
  date: string;
  refs: string[];
  subject: string;
  files: GitCommitFile[];
}

export interface GitOperationResult {
  ok: boolean;
  command: string;
  stdout: string;
  stderr: string;
}

export interface GitCommitMessageDraft {
  project_path: string;
  repo_root: string | null;
  is_repo: boolean;
  source: "staged" | "working_tree" | "none" | string;
  title: string;
  body: string;
  files: GitChangedFile[];
  blocked_reason: string | null;
}

export interface GitPushCommit {
  hash: string;
  full_hash: string;
  author_name: string;
  date: string;
  subject: string;
}

export interface GitPushPreview {
  project_path: string;
  repo_root: string | null;
  is_repo: boolean;
  current_branch: string | null;
  upstream: string | null;
  remote: string | null;
  remote_branch: string | null;
  ahead: number;
  behind: number;
  commits: GitPushCommit[];
  blocked_reason: string | null;
}

export interface GitPushRiskItem {
  severity: "high" | "medium" | "low" | string;
  category: string;
  title: string;
  detail: string;
  file_path: string | null;
}

export interface GitPushRiskScan {
  project_path: string;
  repo_root: string | null;
  is_repo: boolean;
  current_branch: string | null;
  upstream: string | null;
  ahead: number;
  scanned_files: number;
  risks: GitPushRiskItem[];
  blocked_reason: string | null;
}

export interface GitCompareCommit {
  side: "base" | "target" | string;
  hash: string;
  full_hash: string;
  author_name: string;
  date: string;
  subject: string;
}

export interface GitRefCompare {
  project_path: string;
  repo_root: string | null;
  is_repo: boolean;
  base_ref: string;
  target_ref: string;
  merge_base: string | null;
  ahead: number;
  behind: number;
  commits: GitCompareCommit[];
  files: GitCommitFile[];
  blocked_reason: string | null;
}

export interface GitBatchCommitTarget {
  project_path: string;
  message: string | null;
}

export interface GitBatchCommitPreviewItem {
  project_path: string;
  repo_root: string | null;
  is_repo: boolean;
  current_branch: string | null;
  files_changed: number;
  staged_files: number;
  additions: number;
  deletions: number;
  ahead: number;
  behind: number;
  blocked_reason: string | null;
}

export interface GitBatchProjectResult {
  project_path: string;
  current_branch: string | null;
  ok: boolean;
  committed: boolean;
  pushed: boolean;
  skipped: boolean;
  message: string;
  commit_result: GitOperationResult | null;
  push_result: GitOperationResult | null;
}

export interface GitWorktree {
  path: string;
  head: string | null;
  branch: string | null;
  detached: boolean;
  bare: boolean;
}

/** §6.2 per-project workspace trust status. */
export interface ProjectTrust {
  project: string;
  trusted: boolean;
  /** Whether trust enforcement (untrusted → bash approval) is currently active. */
  enforced: boolean;
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

// ===========================================================================
// Skill Marketplace (skillsmp.com + GitHub install flow)
// ---------------------------------------------------------------------------
// Mirrors the Rust types reachable via the `skill_market_*` Tauri commands.
// Field-naming is verified against the actual `#[serde(...)]` attributes:
//   - MarketSkill / Pagination use field-level `rename = "camelCase"` for a
//     handful of keys (the rest stay snake_case).
//   - ScanReport / FileInfo / RiskItem stay snake_case (no crate-wide rename),
//     except `FileInfo` renames its Rust field `kind` to `type` on the wire.
//   - RiskCategory / RiskSeverity / SortBy / SkillsMpKeySource serialize as
//     lowercase string variants (`#[serde(rename_all = "lowercase")]`).
//   - MarketSearchInput is the only marketplace command struct with crate-wide
//     `rename_all = "camelCase"` — so its `sortBy` field is camelCase on the
//     wire.
// ===========================================================================

/** One row from `GET /api/v1/skills/search` (mirrors deepagent-skills::MarketSkill). */
export interface MarketSkill {
  id: string;
  name: string;
  author: string;
  description: string;
  /** GitHub source URL (`https://github.com/{owner}/{repo}/tree/{branch}/{path}`). */
  githubUrl: string;
  /** Canonical skillsmp.com page URL for the "view on web" link. */
  skillUrl: string;
  stars: number;
  /** Last update — unix epoch seconds. */
  updatedAt: number;
}

/** Pagination block from `data.pagination` (mirrors deepagent-skills::Pagination). */
export interface Pagination {
  page: number;
  limit: number;
  total: number;
  hasNext: boolean;
  hasPrev: boolean;
}

/** Search-response body envelope (mirrors deepagent-skills::MarketSearchData). */
export interface MarketSearchData {
  skills: MarketSkill[];
  pagination: Pagination;
}

/** Sort order. Lowercased on the wire. */
export type SortBy = "stars" | "recent";

/** Input to `skillMarketSearch`. Mirrors the desktop-side
 *  `MarketSearchInput` struct, which is `#[serde(rename_all = "camelCase")]`,
 *  so `sortBy` is camelCase on the wire. */
export interface MarketSearchInput {
  q?: string;
  page?: number;
  limit?: number;
  sortBy?: SortBy;
  category?: string;
  occupation?: string;
}

/** Coarse category of a static-scan finding. */
export type RiskCategory =
  | "shell"
  | "execution"
  | "network"
  | "credential"
  | "filesystem"
  | "exfiltration";

/** Severity tier of a static-scan finding. */
export type RiskSeverity = "safe" | "warning" | "danger";

/** One file entry (mirrors deepagent-skills::FileInfo).
 *  The Rust field `kind` is `#[serde(rename = "type")]` on the wire. */
export interface FileInfo {
  name: string;
  /** Lower-cased extension including the leading dot (e.g. `.py`), or `""`. */
  type: string;
  size: number;
}

/** One scan finding (mirrors deepagent-skills::RiskItem). */
export interface RiskItem {
  category: RiskCategory;
  severity: RiskSeverity;
  /** Forward-slashed path relative to the skill root. */
  file: string;
  /** 1-based line number; `null` for file-level rules. */
  line: number | null;
  detail: string;
}

/** Static-scan summary (mirrors deepagent-skills::ScanReport).
 *  All field names are snake_case to match the Rust struct (no rename). */
export interface ScanReport {
  name: string;
  skill_md_content: string;
  files: FileInfo[];
  risks: RiskItem[];
}

/** Result of `skill_market_scan` (mirrors the desktop-side `ScanResult`).
 *  Snake_case on the wire — no crate-wide rename. */
export interface ScanResult {
  temp_id: string;
  report: ScanReport;
}

/** AI security review verdict (mirrors deepagent-app-core::AiReviewResult).
 *  Snake_case on the wire. */
export interface AiReviewResult {
  passed: boolean;
  /** Full LLM output (analysis + verdict line). */
  raw_text: string;
  /** Captured one-line reason after `FAIL:`; `null` on PASS / parse error. */
  failure_reason: string | null;
}

/** Streaming-token event payload (Tauri event `skill-ai-review`). */
export interface SkillAiReviewToken {
  temp_id: string;
  token: string;
}

/** Final review event payload (Tauri event `skill-ai-review-done`).
 *  Exactly one of `result` / `error` is non-null. */
export interface SkillAiReviewDone {
  temp_id: string;
  result: AiReviewResult | null;
  error: string | null;
}

/** Provenance of the SkillsMP API key currently in use.
 *  Lowercased on the wire (mirrors deepagent-skills::ApiKeySource). */
export type SkillsMpKeySource = "user" | "builtin" | "none";

/** Result of `skill_market_get_api_key` (mirrors the desktop-side `ApiKeyInfo`).
 *  Never carries the key value itself. Snake_case on the wire. */
export interface ApiKeyInfo {
  has_user_key: boolean;
  source: SkillsMpKeySource;
}

/** Result of `skill_market_test_key` (mirrors the desktop-side `TestKeyResult`).
 *  Snake_case on the wire. */
export interface TestKeyResult {
  ok: boolean;
  /** `X-RateLimit-Daily-Remaining` from the most recent response, or `null`. */
  daily_remaining: number | null;
  /** Error message when `ok === false`; `null` on success. */
  error: string | null;
}

/** Tool-call result shape disclosed by the `skill` tool (channel B).
 *  Mirrors deepagent-skills::SkillToolOutput. `base_dir` is omitted on the
 *  wire when the skill carries no on-disk root. */
export interface SkillToolOutput {
  id: string;
  name: string;
  body: string;
  base_dir?: string;
  resources: string[];
}

// ---- file preview (office-agent) -----------------------------------------

/** Metadata about a file selected for preview (mirrors PreviewMetadataDto). */
export interface PreviewMetadata {
  path: string;
  name: string;
  /** Lowercased extension without the dot, "" if none. */
  ext: string;
  size_bytes: number;
  /** "text" | "image" | "pdf" | "docx" | "xlsx" | "pptx" | "csv" | "unknown". */
  kind: string;
}

/** One xlsx sheet's preview (first N rows) (mirrors SheetPreviewDto). */
export interface SheetPreview {
  name: string;
  rows: string[][];
  truncated: boolean;
}

/** Result of extracting a previewable representation (mirrors PreviewResultDto). */
export interface PreviewResult {
  metadata: PreviewMetadata;
  text: string | null;
  sheets: SheetPreview[] | null;
  truncated: boolean;
  message: string | null;
}

/** Result of a PDF render request (mirrors PdfRenderResultDto). */
export interface PdfRenderResult {
  rendered: boolean;
  pages: string[];
  text: string | null;
  message: string | null;
}

/** One directory entry for the active project's file browser. */
export interface ProjectFileEntry {
  path: string;
  name: string;
  rel_path: string;
  is_dir: boolean;
  size_bytes: number | null;
  ext: string;
}

/** One lazy-loaded directory listing for the active project. */
export interface ProjectFileListResult {
  root_path: string;
  entries: ProjectFileEntry[];
}

/** Recursive fuzzy-search result for @ file/folder mentions. */
export interface ProjectFileSearchResult {
  root_path: string;
  entries: ProjectFileEntry[];
}

export type ComposerMention =
  | {
      kind: "file" | "folder";
      path: string;
      relPath: string;
      name: string;
      isDir: boolean;
    }
  | {
      kind: "goal";
      text: string;
    }
  | {
      kind: "plan_mode";
    };

// ---- recording + transcription (office-agent) -----------------------------

/** A recording session's lifecycle (mirrors RecordingSessionDto). */
export interface RecordingSession {
  id: string;
  /** "idle" | "recording" | "paused" | "transcribing" | "done" | "error". */
  status: string;
  started_at: number;
  duration_ms: number;
  audio_path: string | null;
  transcript_path: string | null;
  error: string | null;
}

/** One transcript segment (mirrors TranscriptSegmentDto). */
export interface TranscriptSegment {
  start_ms: number;
  end_ms: number;
  text: string;
  speaker: string | null;
  confidence: number | null;
}

// ---- managed runtimes (office-agent) --------------------------------------

/** Status of one managed runtime (mirrors RuntimeStatusDto). */
export interface RuntimeStatus {
  id: string;
  name: string;
  version: string;
  capability: string;
  size_bytes: number;
  installed: boolean;
  available_for_platform: boolean;
  checksum_pinned: boolean;
  install_path: string | null;
  install_source?: "active" | "fallback" | "system" | null;
}

export interface RuntimeRoots {
  active_root: string;
  fallback_roots: string[];
}

/** Progress payload for `runtime:progress` events (mirrors RuntimeProgressDto). */
export interface RuntimeProgress {
  id: string;
  downloaded: number;
  total: number | null;
  /** "downloading" | "verifying" | "extracting" | "done" | "error". */
  phase: string;
}

// --- SSH long-lived connection types (Phase SSH) ---

export type SshAuthType = "agent" | "key_file" | "password";
export type SshStatus = "disconnected" | "connecting" | "connected" | "error";

export interface SshConnection {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth_type: SshAuthType;
  key_path?: string;
  status: SshStatus;
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

export interface SshServiceHandle {
  connection_id: string;
  token: string;
  cols: number;
  rows: number;
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
