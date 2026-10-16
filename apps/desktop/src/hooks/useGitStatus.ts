import { useCallback, useEffect, useState } from "react";
import { gitBranchList, gitChanges, gitProjectStatus } from "../api";
import type { GitBranch, GitChanges, GitProjectStatus } from "../types";

type GitStatusState = {
  projectPath: string | null;
  cacheKey: string | null;
  loading: boolean;
  status: GitProjectStatus | null;
  branches: GitBranch[];
  changes: GitChanges | null;
  error: string | null;
  cachedAt: number;
};

const EMPTY_STATE: GitStatusState = {
  projectPath: null, cacheKey: null, loading: false, status: null, branches: [], changes: null, error: null, cachedAt: 0,
};
const GIT_STATUS_CACHE_PREFIX = "deepagent:git-status:";
// Local freshness policy: navigation reuses recent project data; explicit refresh bypasses it.
const GIT_STATUS_FRESH_MS = 30_000;
const GIT_CLIENT = { gitBranchList, gitChanges, gitProjectStatus };
// Injectable read boundary for fixture/queue tests; production uses the Tauri API.
export type GitStatusClient = typeof GIT_CLIENT;
type GitStatusOptions = {
  /** Git workbenches refresh automatically; conversation summaries refresh on live events. */
  automatic?: boolean;
  /** Undefined uses the project cache, null disables disk caching, a string creates an isolated scope. */
  cacheScope?: string | null;
};

type GitStatusListener = (state: GitStatusState) => void;
type SharedRequest = { promise: Promise<void>; queued: boolean; full: boolean };
const sharedSnapshots = new Map<string, GitStatusState>();
const sharedRequests = new Map<string, SharedRequest>();
const sharedListeners = new Map<string, Set<GitStatusListener>>();
const clientIds = new WeakMap<object, number>();
let nextClientId = 1;

function clientKey(client: GitStatusClient): number {
  const object = client as object;
  const existing = clientIds.get(object);
  if (existing !== undefined) return existing;
  const id = nextClientId++;
  clientIds.set(object, id);
  return id;
}

function sharedKey(projectPath: string, cacheKey: string | null, client: GitStatusClient): string {
  return `${clientKey(client)}:${cacheKey ?? `memory:${projectPath}`}`;
}

function readCache(projectPath: string, cacheKey: string | null): GitStatusState {
  if (!cacheKey || typeof localStorage === "undefined") {
    return { ...EMPTY_STATE, projectPath, cacheKey };
  }
  try {
    const raw = localStorage.getItem(cacheKey);
    const parsed = raw ? JSON.parse(raw) : null;
    if (parsed?.version === 2 && parsed.projectPath === projectPath && Array.isArray(parsed.branches)) {
      return { ...parsed, cacheKey, loading: false, error: null,
        cachedAt: typeof parsed.cachedAt === "number" ? parsed.cachedAt : 0 };
    }
  } catch {
    // Best-effort UI cache.
  }
  return { ...EMPTY_STATE, projectPath, cacheKey };
}

function writeCache(state: GitStatusState, cacheKey: string | null) {
  if (!cacheKey || typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(cacheKey, JSON.stringify({ ...state, version: 2 }));
  } catch {
    // Best-effort UI cache.
  }
}

function getSnapshot(projectPath: string, cacheKey: string | null, client: GitStatusClient): GitStatusState {
  const key = sharedKey(projectPath, cacheKey, client);
  const existing = sharedSnapshots.get(key);
  if (existing) return existing;
  const snapshot = readCache(projectPath, cacheKey);
  sharedSnapshots.set(key, snapshot);
  return snapshot;
}

function initialVisible(snapshot: GitStatusState, automatic: boolean): GitStatusState {
  if (automatic && !snapshot.status && !snapshot.changes && !snapshot.error) {
    return { ...snapshot, loading: true };
  }
  return snapshot;
}

function publish(key: string, snapshot: GitStatusState): void {
  sharedSnapshots.set(key, snapshot);
  sharedListeners.get(key)?.forEach((listener) => listener(snapshot));
}

function subscribe(key: string, listener: GitStatusListener): () => void {
  const listeners = sharedListeners.get(key) ?? new Set<GitStatusListener>();
  listeners.add(listener);
  sharedListeners.set(key, listeners);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) sharedListeners.delete(key);
  };
}

/** One deduplicated refresh queue per project cache key. */
async function performSharedRequest(
  projectPath: string,
  cacheKey: string | null,
  client: GitStatusClient,
  full: boolean,
): Promise<void> {
  const key = sharedKey(projectPath, cacheKey, client);
  const current = getSnapshot(projectPath, cacheKey, client);
  const complete = full || !current.status?.is_repo;
  publish(key, { ...current, loading: true, error: null });
  try {
    const [status, changes] = await Promise.all([
      complete ? client.gitProjectStatus(projectPath) : Promise.resolve(current.status!),
      client.gitChanges(projectPath),
    ]);
    const branches = changes.is_repo
      ? complete ? await client.gitBranchList(projectPath) : current.branches
      : [];
    const snapshot: GitStatusState = {
      projectPath, cacheKey, loading: false, status, branches, changes, error: null, cachedAt: Date.now(),
    };
    writeCache(snapshot, cacheKey);
    publish(key, snapshot);
  } catch (error) {
    publish(key, {
      ...getSnapshot(projectPath, cacheKey, client),
      loading: false,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

function requestShared(
  projectPath: string,
  cacheKey: string | null,
  client: GitStatusClient,
  full: boolean,
): Promise<void> {
  const key = sharedKey(projectPath, cacheKey, client);
  const existing = sharedRequests.get(key);
  if (existing) {
    existing.queued = true;
    existing.full ||= full;
    return existing.promise;
  }
  const record = { promise: Promise.resolve(), queued: false, full } satisfies SharedRequest;
  sharedRequests.set(key, record);
  record.promise = (async () => {
    do {
      record.queued = false;
      const nextFull = record.full;
      record.full = false;
      await performSharedRequest(projectPath, cacheKey, client, nextFull);
    } while (record.queued);
  })().finally(() => {
    if (sharedRequests.get(key) === record) sharedRequests.delete(key);
  });
  return record.promise;
}

function ensureFresh(projectPath: string, cacheKey: string | null, client: GitStatusClient): Promise<void> {
  const pending = sharedRequests.get(sharedKey(projectPath, cacheKey, client));
  if (pending) return pending.promise;
  const snapshot = getSnapshot(projectPath, cacheKey, client);
  const age = Date.now() - snapshot.cachedAt;
  if (!snapshot.error && snapshot.cachedAt > 0 && age >= 0 && age < GIT_STATUS_FRESH_MS) {
    return Promise.resolve();
  }
  return requestShared(projectPath, cacheKey, client, true);
}

export function useGitStatus(
  projectPath?: string | null,
  client: GitStatusClient = GIT_CLIENT,
  { automatic = true, cacheScope }: GitStatusOptions = {},
) {
  const path = projectPath || null;
  const cacheKey = !path || cacheScope === null
    ? null
    : GIT_STATUS_CACHE_PREFIX + encodeURIComponent(path)
      + (cacheScope === undefined ? "" : ":scope:" + encodeURIComponent(cacheScope));
  const initial = path
    ? initialVisible(getSnapshot(path, cacheKey, client), automatic)
    : EMPTY_STATE;
  const [state, setState] = useState<GitStatusState>(initial);
  const refresh = useCallback(
    () => path ? requestShared(path, cacheKey, client, true) : Promise.resolve(),
    [path, cacheKey, client],
  );
  const refreshChanges = useCallback(
    () => path ? requestShared(path, cacheKey, client, false) : Promise.resolve(),
    [path, cacheKey, client],
  );

  useEffect(() => {
    if (!path) {
      setState(EMPTY_STATE);
      return;
    }
    const key = sharedKey(path, cacheKey, client);
    setState(initialVisible(getSnapshot(path, cacheKey, client), automatic));
    const unsubscribe = subscribe(key, setState);
    if (automatic) void ensureFresh(path, cacheKey, client);
    const onFocus = () => {
      if (document.visibilityState === "visible") void ensureFresh(path, cacheKey, client);
    };
    if (automatic) {
      window.addEventListener("focus", onFocus);
      document.addEventListener("visibilitychange", onFocus);
    }
    return () => {
      unsubscribe();
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, [path, cacheKey, client, automatic]);

  // A navigation to another project reads its existing shared snapshot during
  // render, so cached branch data stays visible while the background refresh runs.
  const visible = path
    ? (state.projectPath === path && state.cacheKey === cacheKey
      ? state
      : initialVisible(getSnapshot(path, cacheKey, client), automatic))
    : EMPTY_STATE;
  return { ...visible, refresh, refreshChanges };
}
