import type { HarnessEvent } from "./index.js";

// Local fanout limits, not a replay store or a remote execution budget.
const MAX_BUFFERED_EVENTS = 256;
const MAX_BUFFERED_BYTES = 4 * 1024 * 1024;
type Waiter = {
  resolve: (value: IteratorResult<HarnessEvent>) => void;
  reject: (error: unknown) => void;
};
type Hooks = {
  threadId: string;
  signal?: AbortSignal;
  start: (signal: AbortSignal) => Promise<string>;
  subscribe: (event: (value: HarnessEvent) => void, failure: (error: unknown) => void) => () => void;
  afterTerminal: () => Promise<void>;
  error: (message: string) => Error;
};
const isTerminal = (event: HarnessEvent): boolean =>
  event.type === "turn.completed" || event.type === "turn.failed" || event.type === "turn.interrupted";

/** One lazy, bounded subscription spanning request admission and consumption. */
export class TurnEventStream implements AsyncIterableIterator<HarnessEvent> {
  private readonly controller = new AbortController();
  private readonly values: Array<{ event: HarnessEvent; bytes: number }> = [];
  private readonly waiters: Waiter[] = [];
  private bytes = 0;
  private started = false;
  private turnId?: string;
  private terminal = false;
  private finishing = false;
  private ended = false;
  private failure?: unknown;
  private unsubscribe: () => void = () => {};
  private readonly abort = () => this.fail(this.hooks.error(
    "turn stream cancelled locally; reconcile thread/read before retrying an admitted turn"
  ));

  constructor(private readonly hooks: Hooks) {}

  [Symbol.asyncIterator](): AsyncIterableIterator<HarnessEvent> { return this; }

  next(): Promise<IteratorResult<HarnessEvent>> {
    this.start();
    return new Promise((resolve, reject) => {
      this.waiters.push({ resolve, reject });
      this.drain();
    });
  }

  return(): Promise<IteratorResult<HarnessEvent>> {
    this.ended = true;
    this.failure = undefined;
    this.values.length = 0;
    this.bytes = 0;
    this.release();
    this.drain();
    return Promise.resolve({ done: true, value: undefined });
  }

  throw(error: unknown): Promise<IteratorResult<HarnessEvent>> {
    this.fail(error);
    return Promise.reject(error);
  }

  private start(): void {
    if (this.started || this.ended) return;
    this.started = true;
    if (this.hooks.signal?.aborted) { this.abort(); return; }
    this.hooks.signal?.addEventListener("abort", this.abort, { once: true });
    // Install both event and transport-lifetime listeners before sending.
    try {
      this.unsubscribe = this.hooks.subscribe((event) => this.accept(event), (error) => this.fail(error));
    } catch (error) { this.fail(error); return; }
    if (this.ended) { this.unsubscribe(); return; }
    void this.hooks.start(this.controller.signal).then((turnId) => {
      if (this.ended) return;
      if (typeof turnId !== "string" || !turnId.trim()) {
        this.fail(this.hooks.error("turn/start result lacks a valid turnId"));
        return;
      }
      this.turnId = turnId;
      const candidates = this.values.splice(0);
      this.bytes = 0;
      for (const { event } of candidates) this.accept(event);
      this.drain();
    }, (error) => this.fail(error));
  }

  private accept(event: HarnessEvent): void {
    if (this.ended || this.terminal) return;
    const threadId = "threadId" in event ? event.threadId : undefined;
    const turnId = "turnId" in event ? event.turnId : undefined;
    // v1 DTOs allow absent threadId; a conflicting explicit identity never matches.
    if ((threadId !== undefined && threadId !== this.hooks.threadId) ||
        typeof turnId !== "string" || !turnId.trim() ||
        (this.turnId !== undefined && turnId !== this.turnId)) return;
    const bytes = Buffer.byteLength(JSON.stringify(event), "utf8");
    if (this.values.length >= MAX_BUFFERED_EVENTS || this.bytes + bytes > MAX_BUFFERED_BYTES) {
      this.fail(this.hooks.error("turn stream exceeded its bounded buffer; reconcile thread/read before retrying"));
      return;
    }
    this.values.push({ event, bytes });
    this.bytes += bytes;
    if (this.turnId !== undefined && isTerminal(event)) {
      this.terminal = true;
      this.unsubscribe();
      this.unsubscribe = () => {};
    }
    this.drain();
  }

  private drain(): void {
    if (this.failure !== undefined) {
      while (this.waiters.length) this.waiters.shift()!.reject(this.failure);
      return;
    }
    if (this.ended) {
      while (this.waiters.length) this.waiters.shift()!.resolve({ done: true, value: undefined });
      return;
    }
    if (this.turnId === undefined) return;
    while (this.waiters.length && this.values.length) {
      const { event, bytes } = this.values.shift()!;
      this.bytes -= bytes;
      this.waiters.shift()!.resolve({ done: false, value: event });
    }
    if (this.terminal && !this.values.length && !this.finishing) {
      this.finishing = true;
      // Keep the existing approval completion contract (H-295 is independent).
      void this.hooks.afterTerminal().then(() => {
        if (this.ended) return;
        this.ended = true;
        this.release();
        this.drain();
      }, (error) => this.fail(error));
    }
  }

  private fail(error: unknown): void {
    if (this.ended) return;
    this.failure = error ?? this.hooks.error("turn stream failed");
    this.ended = true;
    this.values.length = 0;
    this.bytes = 0;
    this.release();
    this.drain();
  }

  private release(): void {
    this.unsubscribe();
    this.unsubscribe = () => {};
    this.hooks.signal?.removeEventListener("abort", this.abort);
    this.controller.abort();
  }
}
