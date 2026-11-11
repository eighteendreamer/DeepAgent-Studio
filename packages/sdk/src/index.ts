import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface, type Interface } from "node:readline";
import { TurnEventStream } from "./turnStream.js";
import { CliJsonlTransport } from "./cliJsonl.js";
import { validateControlResult } from "./controlResults.js";
export { HarnessResultError } from "./controlResults.js";

export const PROTOCOL_VERSION = 1;
const JSONRPC_VERSION = "2.0";

export type HarnessEvent =
  | ThreadStartedEvent
  | ItemUpdatedEvent
  | ItemStartedEvent
  | ItemCompletedEvent
  | ApprovalRequestedEvent
  | TurnCompletedEvent
  | TurnFailedEvent
  | TurnInterruptedEvent
  | HarnessErrorEvent;

export interface ThreadStartedEvent {
  type: "thread.started";
  threadId: string;
  title: string | null;
  protocolVersion: number;
}

export interface ItemUpdatedEvent {
  type: "item.updated";
  threadId?: string;
  turnId?: string;
  itemId?: string | null;
  item: ItemPayload;
}

export interface ItemStartedEvent {
  type: "item.started";
  threadId?: string;
  turnId?: string;
  itemId?: string | null;
  item: ItemPayload;
}

export interface ItemCompletedEvent {
  type: "item.completed";
  threadId?: string;
  turnId?: string;
  itemId?: string | null;
  item: ItemPayload;
}

export interface ApprovalRequestedEvent {
  type: "approval.requested";
  approvalId?: string;
  threadId?: string;
  turnId?: string;
  toolName?: string;
  reason: string;
  scope?: string;
}

export interface TurnCompletedEvent {
  type: "turn.completed";
  threadId?: string;
  turnId?: string;
  message: string;
}

export interface TurnFailedEvent {
  type: "turn.failed";
  threadId?: string;
  turnId?: string;
  reason: string;
}

export interface TurnInterruptedEvent {
  type: "turn.interrupted";
  threadId?: string;
  turnId?: string;
  reason?: string;
}

export interface HarnessErrorEvent {
  type: "error";
  code: string;
  message: string;
  data?: unknown;
}

/** Durable, untrusted tool evidence; image bytes are not embedded in events. */
export type ToolResultContent =
  | { type: "text"; text: string }
  | {
      type: "image";
      attachment: {
        id: string;
        kind: "image";
        media_type?: string;
        path?: string;
        sha256?: string;
      };
    };

export type ItemPayload =
  | { kind: "content_delta"; text: string }
  | { kind: "reasoning_delta"; text: string }
  | {
      kind: "tool_call";
      callId: string;
      name: string;
      arguments: unknown;
      toolKind?: string;
      filePath?: string;
      summary?: string;
      meta?: unknown;
    }
  | {
      kind: "tool_result";
      callId: string;
      ok: boolean;
      output: unknown;
      content?: ToolResultContent[];
      durationMs: number;
      toolKind?: string;
      filePath?: string;
      summary?: string;
      meta?: unknown;
    }
  | {
      kind: "usage";
      promptTokens: number;
      completionTokens: number;
      reasoningTokens: number;
      totalTokens: number;
      promptCacheHitTokens: number;
      promptCacheMissTokens: number;
      raw?: unknown;
    }
  | {
      kind: "subagent";
      id: string;
      parentRunId: string;
      state?: string;
      summary?: string;
      background: boolean;
    }
  | { kind: "runtime"; eventType: string; data: unknown };

export interface ThreadStartOptions {
  cwd?: string;
  provider?: string;
  model?: string;
  reasoningEffort?: string;
  permissionProfile?: string;
  sandboxBackend?: string;
}

export interface TurnStartOptions {
  expectedConfigSnapshotId?: string;
  provider?: string;
  model?: string;
  reasoningEffort?: string;
  permissionProfile?: string;
  sandboxBackend?: string;
}

export interface ConfigReadOptions {
  threadId?: string;
  cwd?: string;
}

/** Limited host execution configuration, never raw settings or credentials. */
export interface ExecutionConfigSnapshot {
  configSnapshotVersion: 1;
  configSnapshotId: string;
  resolutionStatus: "resolved" | "uninitialized" | "invalid";
  credentialChecked: false;
  scope: { kind: "global" | "thread" | "cwd"; requiresThreadScope: boolean; threadId?: string };
  provider: string | null;
  model: string | null;
  reasoningEffort: string | null;
  permissionProfile: { approvalPolicy: string; sandboxMode: string; localExecutionMode: string; networkAlwaysAsk: boolean };
  sandboxBackend: string | null;
  [key: string]: unknown;
}

/** Local stream consumption options; signal is never serialized onto the wire. */
export interface RunStreamOptions extends TurnStartOptions {
  signal?: AbortSignal;
}

/** Stops this owned CLI process; never serialized as a provider/wire field. */
export interface CliRunOptions {
  signal?: AbortSignal;
}

export interface ApprovalRequest {
  approvalId: string;
  threadId?: string;
  turnId?: string;
  toolName?: string;
  reason: string;
  scope?: string;
}

export type ApprovalHandler = (
  request: ApprovalRequest
) => boolean | Promise<boolean>;

export interface DeepAgentOptions {
  transport?: "stdio" | "cli-jsonl";
  command?: string;
  args?: readonly string[];
  cwd?: string;
  clientName?: string;
  clientVersion?: string;
  onApproval?: ApprovalHandler;
  onEvent?: (event: HarnessEvent) => void;
  onStderr?: (line: string) => void;
}

export interface ThreadInfo {
  threadId: string;
  title?: string | null;
  cwd?: string | null;
  createdAt?: number;
  updatedAt?: number;
  ended?: boolean;
  archived?: boolean;
}

export interface ThreadListOptions {
  archived?: boolean;
  /** Default 25, range 1..100. */
  limit?: number;
  cursor?: string;
  /** Exact persisted path; never an authorization grant. */
  cwd?: string;
  /** Explicit pinned provider; unknown/default provider is not inferred. */
  provider?: string;
  search?: string;
  updatedAfter?: number;
  sortKey?: "created_at" | "updated_at";
  sortDirection?: "asc" | "desc";
  maxBytes?: number;
}

export interface ThreadListPage {
  threads: ThreadInfo[];
  listPageVersion: 1;
  hasMore: boolean;
  nextCursor: string | null;
}

export interface ThreadReadOptions {
  /** Total items including run envelopes; default/max 500. Atomic groups must fit. */
  pageSize?: number;
  /** Complete server response budget: 4096..1048576 bytes, default 262144. */
  maxBytes?: number;
  /** Opaque read-only checkpoint. Never use a notification eventSequence here. */
  cursor?: string;
  afterSequence?: number;
  sessionAfterSequence?: number;
  runAfterSequence?: number;
  runAfterSequences?: Record<string, number>;
}

export interface RunHistorySlice {
  runId: string;
  state: string;
  terminalKind: string | null;
  events: unknown[];
  actions: unknown[];
  approvals: unknown[];
  artifacts: unknown[];
  graph: { rootRunId: string; nodes: unknown[] };
}

export interface ThreadHistoryPage {
  threadId: string;
  title: string | null;
  cwd: string | null;
  controlProjectionVersion: number;
  cursorMode: "stream_scoped";
  historyPageVersion: 1;
  events: unknown[];
  /** A run may continue on another page; do not replace its prior slices. */
  runEvents: RunHistorySlice[];
  hasMore: boolean;
  nextCursor: string | null;
}

export interface ThreadStartResult {
  threadId: string;
  status: string;
  cwd?: string | null;
  /** null: legacy/non-harness session without a saved snapshot; undefined: older server. */
  configuration?: SessionExecutionConfiguration | null;
}

/** Explicit overrides, not a snapshot of all resolved global/project settings. */
export interface SessionExecutionConfiguration {
  version: 1;
  overrides: {
    provider?: string;
    model?: string;
    reasoning_effort?: string;
    permission_profile?: string;
    sandbox_backend?: string;
  };
}

export interface TurnStartResult {
  threadId: string;
  turnId: string;
  status: string;
  /** Present when the server atomically admitted the actual run capabilities. */
  snapshotId?: string;
  capabilitySnapshot?: unknown;
}

export interface ToolListOptions {
  threadId?: string;
  runId?: string;
  /** Immutable admission identity; not an authorization or wire-generation token. */
  expectedSnapshotId?: string;
}

export interface ToolListResult {
  protocolVersion: number;
  threadId?: string;
  runId?: string;
  snapshotId?: string;
  tools: unknown[];
  capabilitySnapshot?: unknown;
}

export interface TurnInterruptResult {
  threadId: string;
  turnId: string;
  status: string;
}

export class HarnessRpcError extends Error {
  readonly code: number;
  readonly data: unknown;

  constructor(code: number, message: string, data?: unknown) {
    super(message);
    this.name = "HarnessRpcError";
    this.code = code;
    this.data = data;
  }
}

export class HarnessTransportError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "HarnessTransportError";
  }
}

/** Safe handshake diagnostics; no arbitrary peer result is retained or echoed. */
export class HarnessProtocolError extends Error {
  constructor(readonly field: string, readonly expected?: number, readonly actual?: number) {
    super(expected === undefined
      ? `initialize result has an invalid ${field}; connection was not authorized`
      : `incompatible initialize protocol version: expected ${expected}, actual ${actual}`);
    this.name = "HarnessProtocolError";
  }
}

function validateInitializeResult(value: unknown): void {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new HarnessProtocolError("schema");
  }
  const result = value as Record<string, unknown>;
  const version = result.protocolVersion;
  if (typeof version !== "number" || !Number.isSafeInteger(version) || version < 0) {
    throw new HarnessProtocolError("protocolVersion");
  }
  if (version !== PROTOCOL_VERSION) {
    throw new HarnessProtocolError("protocolVersion", PROTOCOL_VERSION, version);
  }
  for (const field of ["serverName", "serverVersion"] as const) {
    if (typeof result[field] !== "string" || !result[field].trim()) {
      throw new HarnessProtocolError(field);
    }
  }
  const capabilities = result.capabilities;
  if (typeof capabilities !== "object" || capabilities === null || Array.isArray(capabilities)) {
    throw new HarnessProtocolError("capabilities");
  }
  for (const field of ["threadLifecycle", "streaming", "interrupt", "steer", "approval", "reconnect"]) {
    if (typeof (capabilities as Record<string, unknown>)[field] !== "boolean") {
      throw new HarnessProtocolError(`capabilities.${field}`);
    }
  }
}

interface RpcResponse {
  jsonrpc: string;
  id?: number | string | null;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

interface RpcNotification {
  jsonrpc: string;
  method: string;
  params?: unknown;
}

interface RpcTransport {
  request(method: string, params: unknown, signal?: AbortSignal): Promise<unknown>;
  onNotification(handler: (notification: RpcNotification) => void): () => void;
  close(): Promise<void>;
}

type PendingRequest = {
  resolve: (result: unknown) => void;
  reject: (error: unknown) => void;
};

class StdioTransport implements RpcTransport {
  private readonly command: string;
  private readonly args: readonly string[];
  private readonly cwd?: string;
  private readonly onStderr?: (line: string) => void;
  private child?: ChildProcessWithoutNullStreams;
  private lines?: Interface;
  private nextId = 1;
  private readonly pending = new Map<number, PendingRequest>();
  private readonly notificationHandlers = new Set<
    (notification: RpcNotification) => void
  >();
  private closed = false;
  private failure?: unknown;
  private ready = false;
  private initializeTask?: Promise<void>;
  private readonly failureHandlers = new Set<(error: unknown) => void>();

  constructor(options: DeepAgentOptions) {
    this.command = options.command ?? "deepagent";
    this.args = options.args ?? ["server", "--transport", "stdio"];
    this.cwd = options.cwd;
    this.onStderr = options.onStderr;
  }

  private ensureStarted(): ChildProcessWithoutNullStreams {
    if (this.closed) {
      throw new HarnessTransportError("stdio transport is closed");
    }
    if (this.failure !== undefined) throw this.failure;
    if (this.child) {
      return this.child;
    }

    const child = spawn(this.command, [...this.args], {
      cwd: this.cwd,
      stdio: ["pipe", "pipe", "pipe"]
    });
    this.child = child;
    this.lines = createInterface({ input: child.stdout });
    this.lines.on("line", (line) => this.handleLine(line));
    this.lines.once("close", () => this.failPending(new HarnessTransportError("app-server stdout closed")));
    child.stdout.once("close", () => this.failPending(new HarnessTransportError("app-server stdout closed")));
    child.stderr.on("data", (chunk: Buffer) => {
      const text = chunk.toString();
      if (this.onStderr) {
        for (const line of text.split(/\r?\n/u)) {
          if (line.length > 0) {
            this.onStderr(line);
          }
        }
      }
    });
    child.stdin.on("error", (error) => this.failPending(
      new HarnessTransportError("app-server stdin failed", { cause: error })
    ));
    child.stdout.on("error", (error) => this.failPending(
      new HarnessTransportError("app-server stdout failed", { cause: error })
    ));
    child.stderr.on("error", (error) => this.failPending(
      new HarnessTransportError("app-server stderr failed", { cause: error })
    ));
    child.once("error", (error) => this.failPending(
      new HarnessTransportError("failed to start app-server", { cause: error })
    ));
    child.once("exit", (code, signal) => {
      this.child = undefined;
      this.lines = undefined;
      this.failPending(
        new HarnessTransportError(
          `app-server exited before completing a request (code=${code ?? "null"}, signal=${signal ?? "null"})`
        )
      );
    });
    return child;
  }

  private handleLine(line: string): void {
    if (this.closed || this.failure !== undefined) return;
    if (!line.trim()) {
      return;
    }
    let message: RpcResponse | RpcNotification;
    try {
      message = JSON.parse(line) as RpcResponse | RpcNotification;
    } catch (error) {
      this.failPending(
        new HarnessTransportError("app-server emitted invalid JSON", { cause: error })
      );
      return;
    }
    // Reject incompatible envelopes before either pending removal or event
    // delivery. Keep the request for a later valid reply or normal close/EOF.
    if (typeof message !== "object" || message === null || message.jsonrpc !== JSONRPC_VERSION) {
      return;
    }
    if ("method" in message && message.method !== undefined) {
      for (const handler of this.notificationHandlers) {
        handler(message as RpcNotification);
      }
      return;
    }
    if (!("id" in message) || message.id === undefined || message.id === null) {
      return;
    }
    if (typeof message.id !== "number") {
      return;
    }
    const request = this.pending.get(message.id);
    if (!request) {
      return;
    }
    this.pending.delete(message.id);
    const response = message as RpcResponse;
    if (response.error) {
      request.reject(
        new HarnessRpcError(
          response.error.code,
          response.error.message,
          response.error.data
        )
      );
    } else {
      request.resolve(response.result);
    }
  }

  private failPending(error: unknown): void {
    if (this.failure !== undefined) return;
    this.failure = error;
    this.ready = false;
    for (const request of this.pending.values()) {
      request.reject(error);
    }
    this.pending.clear();
    for (const handler of this.failureHandlers) handler(error);
  }

  get initialized(): boolean { return this.ready; }

  /** One immutable physical generation, one shared validated handshake. */
  initialize(params: unknown): Promise<void> {
    if (this.closed) return Promise.reject(new HarnessTransportError("stdio transport is closed"));
    if (this.failure !== undefined) return Promise.reject(this.failure);
    this.initializeTask ??= this.request("initialize", params).then((result) => {
      validateInitializeResult(result);
      if (this.closed) throw new HarnessTransportError("stdio transport closed during initialize");
      if (this.failure !== undefined) throw this.failure;
      this.ready = true;
    });
    return this.initializeTask;
  }

  async request(method: string, params: unknown, signal?: AbortSignal): Promise<unknown> {
    if (signal?.aborted) throw new HarnessTransportError("request cancelled locally before send");
    const child = this.ensureStarted();
    const id = this.nextId++;
    const line = `${JSON.stringify({
      jsonrpc: JSONRPC_VERSION,
      id,
      method,
      params
    })}\n`;
    return new Promise((resolve, reject) => {
      const cleanup = () => signal?.removeEventListener("abort", aborted);
      const aborted = () => {
        this.pending.delete(id);
        cleanup();
        reject(new HarnessTransportError("request cancelled locally; reconcile thread/read before retrying"));
      };
      this.pending.set(id, {
        resolve: (value) => { cleanup(); resolve(value); },
        reject: (error) => { cleanup(); reject(error); }
      });
      signal?.addEventListener("abort", aborted, { once: true });
      if (signal?.aborted) { aborted(); return; }
      child.stdin.write(line, (error) => {
        if (error) {
          this.failPending(new HarnessTransportError("failed to write to app-server", { cause: error }));
        }
      });
    });
  }

  onFailure(handler: (error: unknown) => void): () => void {
    this.failureHandlers.add(handler);
    if (this.failure !== undefined) handler(this.failure);
    return () => this.failureHandlers.delete(handler);
  }

  onNotification(handler: (notification: RpcNotification) => void): () => void {
    this.notificationHandlers.add(handler);
    return () => this.notificationHandlers.delete(handler);
  }

  async close(): Promise<void> {
    this.closed = true;
    this.ready = false;
    this.lines?.close();
    this.lines = undefined;
    const child = this.child;
    this.child = undefined;
    this.failPending(new HarnessTransportError("stdio transport closed"));
    if (!child) {
      return;
    }
    if (!child.killed) {
      child.kill();
    }
  }
}

export class DeepAgent {
  private readonly options: DeepAgentOptions;
  private readonly approvalHandler?: ApprovalHandler;
  private readonly eventHandlers = new Set<(event: HarnessEvent) => void>();
  private readonly approvalTasks = new Set<Promise<void>>();
  private transport?: StdioTransport;
  private cliTransport?: CliJsonlTransport;
  private reconnectTask?: Promise<void>;
  private connectionChange = 0;
  private unsubscribeTransport?: () => void;

  constructor(options: DeepAgentOptions = {}) {
    this.options = options;
    this.approvalHandler = options.onApproval;
    if ((options.transport ?? "stdio") === "cli-jsonl") {
      this.cliTransport = new CliJsonlTransport(options, (message, cause) =>
        new HarnessTransportError(message, cause === undefined ? undefined : { cause }));
    } else {
      this.createStdioTransport();
    }
    if (options.onEvent) {
      this.eventHandlers.add(options.onEvent);
    }
  }

  private createStdioTransport(): void {
    const transport = new StdioTransport(this.options);
    this.transport = transport;
    this.unsubscribeTransport = transport.onNotification((notification) => {
      if (this.transport !== transport) return;
      if (notification.method !== "harness/event") {
        return;
      }
      const event = notification.params as HarnessEvent;
      for (const handler of this.eventHandlers) {
        handler(event);
      }
      if (event.type === "approval.requested") {
        const task = this.resolveApproval(event, transport).catch(() => undefined);
        this.approvalTasks.add(task);
        void task.then(() => this.approvalTasks.delete(task));
      }
    });
  }

  private async resolveApproval(event: ApprovalRequestedEvent, transport: StdioTransport): Promise<void> {
    if (!this.approvalHandler || !event.approvalId) {
      return;
    }
    let approved = false;
    try {
      approved = await this.approvalHandler({
        approvalId: event.approvalId,
        threadId: event.threadId,
        turnId: event.turnId,
        toolName: event.toolName,
        reason: event.reason,
        scope: event.scope
      });
    } catch {
      approved = false;
    }
    if (this.transport !== transport) return;
    await this.requestOn(transport, "approval/respond", {
      approvalId: event.approvalId,
      approved,
      scope: event.scope
    });
  }

  /** Explicit snapshot of currently registered auto-approval tasks, across turns
   * and transport generations. Settlement does not assert an approval succeeded;
   * later arrivals need another drain. Local stream completion never calls this.
   */
  async waitForPendingApprovals(): Promise<void> {
    await Promise.all([...this.approvalTasks]);
  }

  private ensureStdio(): StdioTransport {
    if (!this.transport) {
      throw new HarnessTransportError(
        "this SDK operation requires the stdio app-server transport"
      );
    }
    return this.transport;
  }

  private initialize(transport: StdioTransport): Promise<void> {
    return transport.initialize({
      clientName: this.options.clientName ?? "@deepagent/sdk",
      clientVersion: this.options.clientVersion ?? "0.1.0",
      protocolVersion: PROTOCOL_VERSION
    });
  }

  private async request(method: string, params: unknown, signal?: AbortSignal): Promise<unknown> {
    if (this.reconnectTask) await this.reconnectTask;
    return this.requestOn(this.ensureStdio(), method, params, signal);
  }

  private async requestOn(transport: StdioTransport, method: string, params: unknown, signal?: AbortSignal): Promise<unknown> {
    if (signal?.aborted) throw new HarnessTransportError("request cancelled locally before send");
    await this.initialize(transport);
    const result = await transport.request(method, params, signal);
    validateControlResult(method, params, result);
    return result;
  }

  /** @internal Used by the SDK contract tests and transport adapters. */
  async requestForTest(method: string, params: unknown): Promise<unknown> {
    return this.request(method, params);
  }

  async startThread(options: ThreadStartOptions = {}): Promise<Thread> {
    if (this.cliTransport) {
      throw new HarnessTransportError(
        "startThread requires the stdio app-server transport; use runJsonl for CLI JSONL"
      );
    }
    const result = (await this.request("thread/start", options)) as ThreadStartResult;
    return new Thread(this, result.threadId, result.configuration);
  }

  async resumeThread(threadId: string): Promise<Thread> {
    if (this.cliTransport) {
      throw new HarnessTransportError(
        "resumeThread requires the stdio app-server transport"
      );
    }
    const result = (await this.request("thread/resume", { threadId })) as ThreadStartResult;
    return new Thread(this, threadId, result.configuration);
  }

  /** Compatibility: returns only the first page, never the entire catalog.
   * Use listThreadsPage to observe hasMore and continue explicitly. */
  async listThreads(options: Omit<ThreadListOptions, "cursor"> = {}): Promise<ThreadInfo[]> {
    const result = (await this.request("thread/list", options)) as {
      threads: ThreadInfo[];
    };
    return result.threads;
  }

  /** Exactly one bounded page. Empty pages can still have a continuation when
   * sparse provider filtering consumes the 100-candidate scan budget. Catalog
   * mutation invalidates old cursors: refresh explicitly, never auto-restart. */
  async listThreadsPage(options: ThreadListOptions = {}): Promise<ThreadListPage> {
    const result = await this.request("thread/list", options) as Partial<ThreadListPage> | null;
    const rows = result?.threads;
    const valid = result?.listPageVersion === 1 && typeof result.hasMore === "boolean"
      && Array.isArray(rows) && rows.length <= (options.limit ?? 100) && rows.length <= 100
      && (result.hasMore ? typeof result.nextCursor === "string" && !!result.nextCursor.length
        && result.nextCursor.length <= 4096 && result.nextCursor !== options.cursor : result.nextCursor === null)
      && rows.every((row) => row && typeof row.threadId === "string" && !!row.threadId.trim()
        && (row.title === null || typeof row.title === "string")
        && (row.cwd === null || typeof row.cwd === "string")
        && Number.isSafeInteger(row.createdAt) && Number.isSafeInteger(row.updatedAt)
        && typeof row.ended === "boolean" && typeof row.archived === "boolean"
        && (options.archived === undefined || row.archived === options.archived))
      && new Set(rows.map((row) => row.threadId)).size === rows.length;
    if (!valid) throw new HarnessTransportError("server did not return a supported bounded catalog page");
    return result as ThreadListPage;
  }

  /** Archive refuses busy owners; it never cancels or deletes their work. */
  async archiveThread(threadId: string): Promise<{ threadId: string; archived: true }> {
    return await this.request("thread/archive", { threadId }) as { threadId: string; archived: true };
  }

  /** Restore eligibility only; call resumeThread explicitly before new work. */
  async unarchiveThread(threadId: string): Promise<{ threadId: string; archived: false }> {
    return await this.request("thread/unarchive", { threadId }) as { threadId: string; archived: false };
  }

  /** Fetch exactly one bounded page. No automatic accumulation or retry loop. */
  async readThreadPage(threadId: string, options: ThreadReadOptions = {}): Promise<ThreadHistoryPage> {
    const value = await this.request("thread/read", { ...options, threadId });
    const page = value as Partial<ThreadHistoryPage> | null;
    if (!page || page.threadId !== threadId || page.historyPageVersion !== 1
      || page.cursorMode !== "stream_scoped" || typeof page.hasMore !== "boolean"
      || !Array.isArray(page.events) || !Array.isArray(page.runEvents)
      || (page.hasMore ? typeof page.nextCursor !== "string" || !page.nextCursor.length
        : page.nextCursor !== null)
      || !page.runEvents.every((run) => run && typeof run.runId === "string"
        && [run.events, run.actions, run.approvals, run.artifacts].every(Array.isArray)
        && run.graph && Array.isArray(run.graph.nodes))) {
      throw new HarnessTransportError("server did not return a supported bounded history page");
    }
    return page as ThreadHistoryPage;
  }

  async listTools(options: ToolListOptions = {}): Promise<ToolListResult> {
    return (await this.request("tool/list", options)) as ToolListResult;
  }

  /** Global means defaults for the host workspace, including project layers. */
  async readConfig(options: ConfigReadOptions = {}): Promise<ExecutionConfigSnapshot> {
    return (await this.request("config/read", options)) as ExecutionConfigSnapshot;
  }

  runJsonl(prompt: string, options: CliRunOptions = {}): AsyncIterable<HarnessEvent> {
    if (!this.cliTransport) {
      throw new HarnessTransportError(
        "runJsonl requires the cli-jsonl transport"
      );
    }
    return this.cliTransport.run(prompt, options);
  }

  /** Only future live events; for a turn use Thread.runStreamed before admission. */
  subscribe(handler: (event: HarnessEvent) => void): () => void {
    this.eventHandlers.add(handler);
    return () => this.eventHandlers.delete(handler);
  }

  /** @internal One event subscription tied to the current transport generation. */
  subscribeStream(handler: (event: HarnessEvent) => void, failure: (error: unknown) => void): () => void {
    const transport = this.ensureStdio();
    const offEvents = this.subscribe(handler);
    const offFailure = transport.onFailure(failure);
    return () => { offEvents(); offFailure(); };
  }

  /** Explicitly replace a terminal stdio generation; no business request is replayed.
   * Concurrent reconnects coalesce. Await this before creating a new owned stream;
   * then resume the desired thread and reconcile durable state as appropriate.
   */
  async reconnect(): Promise<void> {
    if (!this.transport) {
      throw new HarnessTransportError("reconnect requires the stdio transport");
    }
    if (this.reconnectTask) return this.reconnectTask;
    const previous = this.transport;
    const change = ++this.connectionChange;
    const task = Promise.resolve().then(async () => {
      this.unsubscribeTransport?.();
      this.unsubscribeTransport = undefined;
      await previous.close();
      if (this.connectionChange !== change) throw new HarnessTransportError("reconnect superseded by close");
      this.createStdioTransport();
    });
    this.reconnectTask = task;
    try { await task; }
    finally { if (this.reconnectTask === task) this.reconnectTask = undefined; }
  }

  async close(): Promise<void> {
    ++this.connectionChange;
    // A later deliberate reconnect is a new intent, not another waiter on the
    // superseded transition. Its finally must not clear a newer task either.
    this.reconnectTask = undefined;
    this.unsubscribeTransport?.();
    this.unsubscribeTransport = undefined;
    if (this.transport) {
      await this.transport.close();
    }
    await this.cliTransport?.close();
  }

  async startTurn(
    threadId: string,
    input: string,
    options: TurnStartOptions = {},
    signal?: AbortSignal
  ): Promise<Turn> {
    const result = (await this.request("turn/start", {
      threadId,
      input,
      ...options
    }, signal)) as TurnStartResult;
    return new Turn(this, threadId, result.turnId, result.snapshotId, result.capabilitySnapshot);
  }

  async interrupt(
    threadId: string,
    turnId: string
  ): Promise<TurnInterruptResult> {
    return (await this.request("turn/interrupt", {
      threadId,
      turnId
    })) as TurnInterruptResult;
  }

  async steer(threadId: string, turnId: string, input: string): Promise<Turn> {
    const result = (await this.request("turn/steer", {
      threadId,
      turnId,
      input
    })) as TurnStartResult & { replacesTurnId?: string };
    return new Turn(this, threadId, result.turnId, result.snapshotId, result.capabilitySnapshot);
  }

  async approvalRespond(
    approvalId: string,
    approved: boolean,
    scope?: string
  ): Promise<unknown> {
    return this.request("approval/respond", { approvalId, approved, scope });
  }
}

export class Thread {
  constructor(
    private readonly agent: DeepAgent,
    public readonly id: string,
    public readonly configuration?: SessionExecutionConfiguration | null
  ) {}

  async run(input: string, options: TurnStartOptions = {}): Promise<Turn> {
    return this.agent.startTurn(this.id, input, options);
  }

  async listTools(runId?: string): Promise<ToolListResult> {
    return this.agent.listTools({ threadId: this.id, runId });
  }

  /**
   * Subscribe before admission and retain a bounded pre-response event window.
   * return()/signal stop local consumption only; an admitted turn may still run.
   * A matched terminal ends this view, independently of auto-approval callbacks.
   */
  runStreamed(input: string, options: RunStreamOptions = {}): AsyncIterableIterator<HarnessEvent> {
    const { signal, ...turnOptions } = options;
    return new TurnEventStream({
      threadId: this.id,
      signal,
      start: async (localSignal) => (await this.agent.startTurn(this.id, input, turnOptions, localSignal)).id,
      subscribe: (event, failure) => this.agent.subscribeStream(event, failure),
      error: (message) => new HarnessTransportError(message)
    });
  }
}

export class Turn {
  constructor(
    private readonly agent: DeepAgent,
    public readonly threadId: string,
    public readonly id: string,
    public readonly snapshotId?: string,
    public readonly capabilitySnapshot?: unknown
  ) {}

  interrupt(): Promise<TurnInterruptResult> {
    return this.agent.interrupt(this.threadId, this.id);
  }

  steer(input: string): Promise<Turn> {
    return this.agent.steer(this.threadId, this.id, input);
  }
}
