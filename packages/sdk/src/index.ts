import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface, type Interface } from "node:readline";

export const PROTOCOL_VERSION = 1;

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
  permissionProfile?: string;
  sandboxBackend?: string;
}

export interface TurnStartOptions {
  provider?: string;
  model?: string;
  reasoningEffort?: string;
  permissionProfile?: string;
  sandboxBackend?: string;
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
}

export interface ThreadStartResult {
  threadId: string;
  status: string;
  cwd?: string | null;
}

export interface TurnStartResult {
  threadId: string;
  turnId: string;
  status: string;
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

class AsyncEventQueue<T> implements AsyncIterable<T> {
  private readonly pending: Array<{
    resolve: (result: IteratorResult<T>) => void;
    reject: (error: unknown) => void;
  }> = [];
  private readonly values: T[] = [];
  private finished = false;
  private failure: unknown;

  push(value: T): void {
    if (this.finished) {
      return;
    }
    const waiter = this.pending.shift();
    if (waiter) {
      waiter.resolve({ done: false, value });
    } else {
      this.values.push(value);
    }
  }

  end(): void {
    if (this.finished) {
      return;
    }
    this.finished = true;
    while (this.pending.length > 0) {
      this.pending.shift()?.resolve({ done: true, value: undefined });
    }
  }

  fail(error: unknown): void {
    if (this.finished) {
      return;
    }
    this.failure = error;
    this.finished = true;
    while (this.pending.length > 0) {
      this.pending.shift()?.reject(error);
    }
  }

  async next(): Promise<IteratorResult<T>> {
    const value = this.values.shift();
    if (value !== undefined) {
      return { done: false, value };
    }
    if (this.failure !== undefined) {
      throw this.failure;
    }
    if (this.finished) {
      return { done: true, value: undefined };
    }
    return new Promise((resolve, reject) => {
      this.pending.push({ resolve, reject });
    });
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return this;
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
  request(method: string, params: unknown): Promise<unknown>;
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
    child.once("error", (error) => this.failPending(error));
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
    for (const request of this.pending.values()) {
      request.reject(error);
    }
    this.pending.clear();
  }

  async request(method: string, params: unknown): Promise<unknown> {
    const child = this.ensureStarted();
    const id = this.nextId++;
    const line = `${JSON.stringify({
      jsonrpc: "2.0",
      id,
      method,
      params
    })}\n`;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      child.stdin.write(line, (error) => {
        if (error) {
          this.pending.delete(id);
          reject(new HarnessTransportError("failed to write to app-server", { cause: error }));
        }
      });
    });
  }

  onNotification(handler: (notification: RpcNotification) => void): () => void {
    this.notificationHandlers.add(handler);
    return () => this.notificationHandlers.delete(handler);
  }

  async close(): Promise<void> {
    this.closed = true;
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

class CliJsonlTransport {
  private readonly command: string;
  private readonly args: readonly string[];
  private readonly cwd?: string;
  private readonly onStderr?: (line: string) => void;

  constructor(options: DeepAgentOptions) {
    this.command = options.command ?? "deepagent";
    this.args = options.args ?? [];
    this.cwd = options.cwd;
    this.onStderr = options.onStderr;
  }

  async *run(prompt: string): AsyncIterable<HarnessEvent> {
    const child = spawn(
      this.command,
      [...this.args, "run", "--json", prompt],
      {
        cwd: this.cwd,
        stdio: ["ignore", "pipe", "pipe"]
      }
    );
    const stderr = createInterface({ input: child.stderr });
    stderr.on("line", (line) => this.onStderr?.(line));
    const lines = createInterface({ input: child.stdout });
    let exitError: Error | undefined;
    child.once("error", (error) => {
      exitError = new HarnessTransportError("failed to start CLI JSONL process", {
        cause: error
      });
    });
    for await (const line of lines) {
      if (!line.trim()) {
        continue;
      }
      try {
        yield JSON.parse(line) as HarnessEvent;
      } catch (error) {
        throw new HarnessTransportError("CLI emitted invalid JSONL", { cause: error });
      }
    }
    stderr.close();
    if (exitError) {
      throw exitError;
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
  private initialized = false;
  private unsubscribeTransport?: () => void;

  constructor(options: DeepAgentOptions = {}) {
    this.options = options;
    this.approvalHandler = options.onApproval;
    if ((options.transport ?? "stdio") === "cli-jsonl") {
      this.cliTransport = new CliJsonlTransport(options);
    } else {
      this.createStdioTransport();
    }
    if (options.onEvent) {
      this.eventHandlers.add(options.onEvent);
    }
  }

  private createStdioTransport(): void {
    this.transport = new StdioTransport(this.options);
    this.unsubscribeTransport = this.transport.onNotification((notification) => {
      if (notification.method !== "harness/event") {
        return;
      }
      const event = notification.params as HarnessEvent;
      for (const handler of this.eventHandlers) {
        handler(event);
      }
      if (event.type === "approval.requested") {
        const task = this.resolveApproval(event).catch(() => undefined);
        this.approvalTasks.add(task);
        void task.then(() => this.approvalTasks.delete(task));
      }
    });
  }

  private async resolveApproval(event: ApprovalRequestedEvent): Promise<void> {
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
    await this.request("approval/respond", {
      approvalId: event.approvalId,
      approved,
      scope: event.scope
    });
  }

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

  private async initialize(): Promise<void> {
    if (this.initialized) {
      return;
    }
    await this.ensureStdio().request("initialize", {
      clientName: this.options.clientName ?? "@deepagent/sdk",
      clientVersion: this.options.clientVersion ?? "0.1.0",
      protocolVersion: PROTOCOL_VERSION
    });
    this.initialized = true;
  }

  private async request(method: string, params: unknown): Promise<unknown> {
    await this.initialize();
    return this.ensureStdio().request(method, params);
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
    return new Thread(this, result.threadId);
  }

  async resumeThread(threadId: string): Promise<Thread> {
    if (this.cliTransport) {
      throw new HarnessTransportError(
        "resumeThread requires the stdio app-server transport"
      );
    }
    await this.request("thread/resume", { threadId });
    return new Thread(this, threadId);
  }

  async listThreads(): Promise<ThreadInfo[]> {
    const result = (await this.request("thread/list", {})) as {
      threads: ThreadInfo[];
    };
    return result.threads;
  }

  runJsonl(prompt: string): AsyncIterable<HarnessEvent> {
    if (!this.cliTransport) {
      throw new HarnessTransportError(
        "runJsonl requires the cli-jsonl transport"
      );
    }
    return this.cliTransport.run(prompt);
  }

  subscribe(handler: (event: HarnessEvent) => void): () => void {
    this.eventHandlers.add(handler);
    return () => this.eventHandlers.delete(handler);
  }

  async reconnect(): Promise<void> {
    if (!this.transport) {
      throw new HarnessTransportError("reconnect requires the stdio transport");
    }
    this.unsubscribeTransport?.();
    await this.transport.close();
    this.initialized = false;
    this.createStdioTransport();
  }

  async close(): Promise<void> {
    this.unsubscribeTransport?.();
    this.unsubscribeTransport = undefined;
    if (this.transport) {
      await this.transport.close();
    }
  }

  async startTurn(
    threadId: string,
    input: string,
    options: TurnStartOptions = {}
  ): Promise<Turn> {
    const result = (await this.request("turn/start", {
      threadId,
      input,
      ...options
    })) as TurnStartResult;
    return new Turn(this, threadId, result.turnId);
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
    return new Turn(this, threadId, result.turnId);
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
    public readonly id: string
  ) {}

  async run(input: string, options: TurnStartOptions = {}): Promise<Turn> {
    return this.agent.startTurn(this.id, input, options);
  }

  async *runStreamed(
    input: string,
    options: TurnStartOptions = {}
  ): AsyncIterable<HarnessEvent> {
    const queue = new AsyncEventQueue<HarnessEvent>();
    let turnId: string | undefined;
    const unsubscribe = this.agent.subscribe((event) => {
      const eventTurnId = "turnId" in event ? event.turnId : undefined;
      if (turnId && eventTurnId === turnId) {
        queue.push(event);
      }
    });
    try {
      const turn = await this.run(input, options);
      turnId = turn.id;
      for await (const event of queue) {
        yield event;
        if (
          (event.type === "turn.completed" ||
            event.type === "turn.failed" ||
            event.type === "turn.interrupted") &&
          event.turnId === turnId
        ) {
          await this.agent.waitForPendingApprovals();
          queue.end();
        }
      }
    } finally {
      unsubscribe();
      queue.end();
    }
  }
}

export class Turn {
  constructor(
    private readonly agent: DeepAgent,
    public readonly threadId: string,
    public readonly id: string
  ) {}

  interrupt(): Promise<TurnInterruptResult> {
    return this.agent.interrupt(this.threadId, this.id);
  }

  steer(input: string): Promise<Turn> {
    return this.agent.steer(this.threadId, this.id, input);
  }
}
