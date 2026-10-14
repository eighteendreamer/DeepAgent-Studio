import { useCallback, useEffect, useRef, useState } from "react";
import { gitBranchList, gitChanges, gitProjectStatus } from "../api";
import type { GitBranch, GitChanges, GitProjectStatus } from "../types";

type GitStatusState = {
  projectPath: string | null;
  loading: boolean;
  status: GitProjectStatus | null;
  branches: GitBranch[];
  changes: GitChanges | null;
  error: string | null;
};
const EMPTY_STATE: GitStatusState = {
  projectPath: null, loading: false, status: null, branches: [], changes: null, error: null,
};
const GIT_STATUS_CACHE_PREFIX = "deepagent:git-status:";
const GIT_CLIENT = { gitBranchList, gitChanges, gitProjectStatus };
// Injectable read boundary for fixture/queue tests; production uses the Tauri API.
export type GitStatusClient = typeof GIT_CLIENT;

function readCache(projectPath: string): GitStatusState {
  try {
    const raw = localStorage.getItem(GIT_STATUS_CACHE_PREFIX + encodeURIComponent(projectPath));
    const parsed = raw ? JSON.parse(raw) : null;
    if (parsed?.version === 2 && parsed.projectPath === projectPath && Array.isArray(parsed.branches)) {
      return { ...parsed, loading: false, error: null };
    }
  } catch { /* Best-effort UI cache. */ }
  return { ...EMPTY_STATE, projectPath };
}

function writeCache(state: GitStatusState) {
  if (!state.projectPath) return;
  try {
    localStorage.setItem(GIT_STATUS_CACHE_PREFIX + encodeURIComponent(state.projectPath),
      JSON.stringify({ ...state, version: 2, cachedAt: Date.now() }));
  } catch { /* Best-effort UI cache. */ }
}

/** One refresh queue per project. Late results cannot update another project. */
export function useGitStatus(projectPath?: string | null, client: GitStatusClient = GIT_CLIENT) {
  const path = projectPath || null;
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
    let snapshot = readCache(path);
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
            snapshot = { projectPath: path, loading: false, status, branches, changes, error: null };
            writeCache(snapshot);
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
    // Cache is a preview; it never replaces checking the actual repository.
    void request(true);
    const onFocus = () => { if (document.visibilityState === "visible") void request(true); };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    return () => {
      cancelled = true;
      refreshRef.current = async () => {};
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, [path, client]);

  // Hide the previous project's data before the new effect even starts.
  const visible = state.projectPath === path ? state : { ...EMPTY_STATE, projectPath: path, loading: !!path };
  return { ...visible, refresh, refreshChanges };
}
