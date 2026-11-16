import type { SessionDetail } from "./types";

/** One speculative history read, shared with the first navigation that needs it.
 * Consumed once, revision/age checked, and never cached after a failed read.
 */
export function createSessionPrefetch(load: (id: string) => Promise<SessionDetail>) {
  let entry: { id: string; revision: number; at: number; promise: Promise<SessionDetail> } | null = null;
  const matches = (id: string, revision: number) => entry?.id === id
    && entry.revision === revision && Date.now() - entry.at < 30_000;
  return {
    warm(id: string, revision: number) {
      if (matches(id, revision)) return;
      const pending = { id, revision, at: Date.now(), promise: load(id) };
      entry = pending;
      void pending.promise.catch(() => { if (entry === pending) entry = null; });
    },
    take(id: string, revision: number) {
      const promise = matches(id, revision) ? entry!.promise : load(id);
      entry = null;
      return promise;
    },
  };
}
