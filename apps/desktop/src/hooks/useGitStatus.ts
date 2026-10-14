import { useCallback, useEffect, useRef, useState } from "react";
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
};
const EMPTY_STATE: GitStatusState = {
  projectPath: null, cacheKey: null, loading: false, status: null, branches: [], changes: null, error: null,
};
const GIT_STATUS_CACHE_PREFIX = "deepagent:git-status:";
const GIT_CLIENT = { gitBranchList, gitChanges, gitProjectStatus };
// Injectable read boundary for fixture/queue tests; production uses the Tauri API.
export type GitStatusClient = typeof GIT_CLIENT;
type GitStatusOptions = {
  /** Git workbenches refresh automatically; conversation summaries refresh on live events. */
  automatic?: boolean;
  /** Undefined uses the project cache, null disables caching, a string isolates a conversation. */
  cacheScope?: string | null;
};

function readCache(projectPath: string, cacheKey: string | null): GitStatusState {
  try {
    const raw = cacheKey ? localStorage.getItem(cacheKey) : null;
    const parsed = raw ? JSON.parse(raw) : null;
    if (parsed?.version === 2 && parsed.projectPath === projectPath && Array.isArray(parsed.branches)) {
      return { ...parsed, cacheKey, loading: false, error: null };
    }
  } catch { /* Best-effort UI cache. */ }
  return { ...EMPTY_STATE, projectPath, cacheKey };
}

function writeCache(state: GitStatusState, cacheKey: string | null) {
  if (!cacheKey) return;
  try {
    localStorage.setItem(cacheKey,
      JSON.stringify({ ...state, version: 2, cachedAt: Date.now() }));
  } catch { /* Best-effort UI cache. */ }
}

/** One refresh queue per project. Late results cannot update another project. */
export function useGitStatus(
  projectPath?: string | null,
  client: GitStatusClient = GIT_CLIENT,
  { automatic = true, cacheScope }: GitStatusOptions = {},
) {
  const path = projectPath || null;
  const cacheKey = !path || cacheScope === null ? null
    : GIT_STATUS_CACHE_PREFIX + encodeURIComponent(path)
      + (cacheScope === undefined ? "" : ":scope:" + encodeURIComponent(cacheScope));
  const [state, setState] = useState<GitStatusState>(EMPTY_STATE);
  const refreshRef = useRef<(full: boolean) => Promise<void>>(async () => {});
  const refresh = useCallback(() => refreshRef.current(true), []);
  const refreshChanges = useCallback(() => refreshRef.current(false), []);

  useEffect(() => {
    if (!path) {
      setState(EMPTY_STATE);
      refreshRef.current = async () => {};
      return;
    }
    let cancelled = false;
    let snapshot = readCache(path, cacheKey);
    let inFlight: Promise<void> | null = null;
    let queued = false;
    let needsFull = false;
    setState(snapshot);
    const request = (full: boolean): Promise<void> => {
      queued = true;
      needsFull ||= full;
      if (inFlight) return inFlight;
      const execute = async () => {
        while (queued && !cancelled) {
          const complete = needsFull || !snapshot.status?.is_repo;
          queued = false;
          needsFull = false;
          setState({ ...snapshot, loading: true, error: null });
          try {
            const [status, changes] = await Promise.all([
              complete ? client.gitProjectStatus(path) : Promise.resolve(snapshot.status!), client.gitChanges(path),
            ]);
            if (cancelled) return;
            const branches = changes.is_repo
              ? complete ? await client.gitBranchList(path) : snapshot.branches : [];
            if (cancelled) return;
            snapshot = { projectPath: path, cacheKey, loading: false, status, branches, changes, error: null };
            writeCache(snapshot, cacheKey);
            setState(snapshot);
          } catch (error) {
            if (cancelled) return;
            snapshot = { ...snapshot, loading: false, error: error instanceof Error ? error.message : String(error) };
            setState(snapshot);
          }
        }
      };
      inFlight = execute().finally(() => { inFlight = null; });
      return inFlight;
    };
    refreshRef.current = request;
    // Conversation navigation only restores its snapshot; live events request fresh data.
    if (automatic) void request(true);
    const onFocus = () => { if (document.visibilityState === "visible") void request(true); };
    if (automatic) {
      window.addEventListener("focus", onFocus);
      document.addEventListener("visibilitychange", onFocus);
    }
    return () => {
      cancelled = true;
      refreshRef.current = async () => {};
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, [path, client, automatic, cacheKey]);

  // Hide the previous project's data before the new effect even starts.
  const visible = state.projectPath === path && state.cacheKey === cacheKey ? state
    : { ...EMPTY_STATE, projectPath: path, cacheKey, loading: automatic && !!path };
  return { ...visible, refresh, refreshChanges };
}
