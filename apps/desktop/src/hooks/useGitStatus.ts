import { useCallback, useEffect, useState } from "react";
import { gitBranchList, gitChanges, gitProjectStatus } from "../api";
import type { GitBranch, GitChanges, GitProjectStatus } from "../types";

type GitStatusState = {
  loading: boolean;
  status: GitProjectStatus | null;
  branches: GitBranch[];
  changes: GitChanges | null;
  error: string | null;
};

const EMPTY_STATE: GitStatusState = {
  loading: false,
  status: null,
  branches: [],
  changes: null,
  error: null,
};

type GitStatusCache = {
  version: 1;
  projectPath: string;
  cachedAt: number;
  status: GitProjectStatus | null;
  branches: GitBranch[];
  changes: GitChanges | null;
};

const GIT_STATUS_CACHE_PREFIX = "deepagent:git-status:";

function gitStatusCacheKey(projectPath: string): string {
  return `${GIT_STATUS_CACHE_PREFIX}${encodeURIComponent(projectPath)}`;
}

function readGitStatusCache(projectPath?: string | null): GitStatusCache | null {
  if (!projectPath || typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(gitStatusCacheKey(projectPath));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<GitStatusCache>;
    if (parsed.version !== 1 || parsed.projectPath !== projectPath || !Array.isArray(parsed.branches)) return null;
    return parsed as GitStatusCache;
  } catch {
    return null;
  }
}

function writeGitStatusCache(
  projectPath: string,
  status: GitProjectStatus | null,
  branches: GitBranch[],
  changes: GitChanges | null,
) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      gitStatusCacheKey(projectPath),
      JSON.stringify({ version: 1, projectPath, cachedAt: Date.now(), status, branches, changes } satisfies GitStatusCache),
    );
  } catch {
    // Best-effort UI cache.
  }
}

export function useGitStatus(projectPath?: string | null) {
  const [state, setState] = useState<GitStatusState>(() => {
    const cached = readGitStatusCache(projectPath);
    if (!cached) return EMPTY_STATE;
    return {
      loading: false,
      status: cached.status,
      branches: cached.branches,
      changes: cached.changes,
      error: null,
    };
  });

  const refresh = useCallback(async () => {
    if (!projectPath) {
      setState(EMPTY_STATE);
      return;
    }

    setState((prev) => ({ ...prev, loading: true, error: null }));
    try {
      const status = await gitProjectStatus(projectPath);
      if (!status.is_repo) {
        writeGitStatusCache(projectPath, status, [], null);
        setState({
          loading: false,
          status,
          branches: [],
          changes: null,
          error: null,
        });
        return;
      }

      const [branches, changes] = await Promise.all([
        gitBranchList(projectPath),
        gitChanges(projectPath),
      ]);
      setState({
        loading: false,
        status,
        branches,
        changes,
        error: null,
      });
      writeGitStatusCache(projectPath, status, branches, changes);
    } catch (error) {
      setState((prev) => ({
        ...prev,
        loading: false,
        error: error instanceof Error ? error.message : String(error),
      }));
    }
  }, [projectPath]);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      if (!projectPath) {
        setState(EMPTY_STATE);
        return;
      }
      const cached = readGitStatusCache(projectPath);
      if (cached) {
        setState({
          loading: false,
          status: cached.status,
          branches: cached.branches,
          changes: cached.changes,
          error: null,
        });
        return;
      }
      setState((prev) => ({ ...prev, loading: true, error: null }));
      try {
        const status = await gitProjectStatus(projectPath);
        if (cancelled) return;
        if (!status.is_repo) {
          writeGitStatusCache(projectPath, status, [], null);
          setState({ loading: false, status, branches: [], changes: null, error: null });
          return;
        }
        const [branches, changes] = await Promise.all([
          gitBranchList(projectPath),
          gitChanges(projectPath),
        ]);
        if (cancelled) return;
        writeGitStatusCache(projectPath, status, branches, changes);
        setState({ loading: false, status, branches, changes, error: null });
      } catch (error) {
        if (cancelled) return;
        setState((prev) => ({
          ...prev,
          loading: false,
          error: error instanceof Error ? error.message : String(error),
        }));
      }
    };
    load();
    return () => {
      cancelled = true;
    };
  }, [projectPath]);

  return {
    ...state,
    refresh,
  };
}
