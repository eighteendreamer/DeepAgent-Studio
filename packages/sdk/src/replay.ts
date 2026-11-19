import { setTimeout } from "node:timers/promises";
import type { DeepAgent, ThreadReadOptions } from "./index.js";

/** Persist after processing a batch. Connection eventSequence is never valid here. */
export interface ReplayCheckpoint {
  threadId?: string;
  sessionAfterSequence?: number;
  runAfterSequences: Record<string, number>;
}

export interface ReplayEvent {
  runId?: string;
  sequence: number;
  event: unknown;
}

export interface ReplayBatch {
  events: ReplayEvent[];
  checkpoint: ReplayCheckpoint;
}

export interface ReplayOptions {
  checkpoint?: ReplayCheckpoint;
  signal?: AbortSignal;
}

function sequence(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Error("thread/read replay requires a safe unsigned sequence");
  }
  return value;
}

/** Local design: pull-driven durable replay and 250ms follow, using only thread/read.
 * This yields session/run events; mutable action/approval/artifact snapshots remain
 * available through readThreadPage. No automatic reconnect, mutation or event ACK.
 */
export async function* watchThreadEvents(
  agent: Pick<DeepAgent, "readThreadPage">,
  threadId: string,
  options: ReplayOptions = {}
): AsyncGenerator<ReplayBatch> {
  if (options.checkpoint?.threadId !== undefined && options.checkpoint.threadId !== threadId) {
    throw new Error("thread/read replay checkpoint belongs to another thread");
  }
  let checkpoint: ReplayCheckpoint = {
    threadId,
    sessionAfterSequence: options.checkpoint?.sessionAfterSequence,
    runAfterSequences: { ...options.checkpoint?.runAfterSequences }
  };
  if (checkpoint.sessionAfterSequence !== undefined) sequence(checkpoint.sessionAfterSequence);
  for (const value of Object.values(checkpoint.runAfterSequences)) sequence(value);
  let cursor: string | undefined;
  let caughtUp = false;
  while (true) {
    options.signal?.throwIfAborted();
    if (caughtUp) await setTimeout(250, undefined, { signal: options.signal });
    const filters: ThreadReadOptions = cursor ? { cursor } : {
      sessionAfterSequence: checkpoint.sessionAfterSequence,
      runAfterSequences: checkpoint.runAfterSequences
    };
    const page = await agent.readThreadPage(threadId, filters);
    options.signal?.throwIfAborted();
    if (page.threadId !== threadId || page.historyPageVersion !== 1
      || page.cursorMode !== "stream_scoped" || typeof page.hasMore !== "boolean"
      || !Array.isArray(page.events) || !Array.isArray(page.runEvents)
      || (page.hasMore ? !page.nextCursor || page.nextCursor === cursor : page.nextCursor !== null)) {
      throw new Error("thread/read replay received an invalid history page");
    }
    const next: ReplayCheckpoint = {
      threadId,
      sessionAfterSequence: checkpoint.sessionAfterSequence,
      runAfterSequences: { ...checkpoint.runAfterSequences }
    };
    const events: ReplayEvent[] = [];
    const collect = (rows: unknown[], runId?: string) => {
      if (!Array.isArray(rows)) throw new Error("thread/read replay requires event rows");
      let previous: number | undefined;
      for (const event of rows) {
        if (!event || typeof event !== "object" || !("streamSequence" in event)) {
          throw new Error("thread/read replay requires a durable stream sequence");
        }
        if (!("stream" in event) || event.stream !== (runId === undefined ? "session" : "run")
          || (runId !== undefined && (!("runId" in event) || event.runId !== runId))) {
          throw new Error("thread/read replay received a row from another stream");
        }
        const current = sequence(event.streamSequence);
        if (previous !== undefined && current <= previous) throw new Error("thread/read replay rows are not ordered");
        previous = current;
        const high = runId === undefined ? next.sessionAfterSequence : next.runAfterSequences[runId];
        if (high !== undefined && current <= high) continue;
        events.push({ runId, sequence: current, event });
        if (runId === undefined) next.sessionAfterSequence = current;
        else Object.defineProperty(next.runAfterSequences, runId, { value: current, enumerable: true, configurable: true, writable: true });
      }
    };
    collect(page.events);
    for (const run of page.runEvents) {
      if (typeof run.runId !== "string" || !run.runId) throw new Error("thread/read replay requires run identity");
      collect(run.events, run.runId);
    }
    checkpoint = next;
    caughtUp = !page.hasMore;
    cursor = page.hasMore ? page.nextCursor! : undefined;
    if (events.length) yield { events, checkpoint: structuredClone(checkpoint) };
  }
}
