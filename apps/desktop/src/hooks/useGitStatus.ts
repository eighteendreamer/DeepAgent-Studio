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

export function useGitStatus(projectPath?: string | null) {
  const [state, setState] = useState<GitStatusState>(EMPTY_STATE);

  const refresh = useCallback(async () => {
    if (!projectPath) {
      setState(EMPTY_STATE);
      return;
    }

    setState((prev) => ({ ...prev, loading: true, error: null }));
    try {
      const status = await gitProjectStatus(projectPath);
      if (!status.is_repo) {
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
      setState((prev) => ({ ...prev, loading: true, error: null }));
      try {
        const status = await gitProjectStatus(projectPath);
        if (cancelled) return;
        if (!status.is_repo) {
          setState({ loading: false, status, branches: [], changes: null, error: null });
          return;
        }
        const [branches, changes] = await Promise.all([
          gitBranchList(projectPath),
          gitChanges(projectPath),
        ]);
        if (cancelled) return;
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
