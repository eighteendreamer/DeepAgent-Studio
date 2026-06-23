import { useCallback, useEffect, useState } from "react";
import { gitProjectsStatus, listProjects } from "../api";
import type { GitProjectStatus, Project } from "../types";

interface GitProjectsState {
  loading: boolean;
  projects: Project[];
  statuses: GitProjectStatus[];
  error: string | null;
}

const EMPTY_STATE: GitProjectsState = {
  loading: false,
  projects: [],
  statuses: [],
  error: null,
};

export function useGitProjects() {
  const [state, setState] = useState<GitProjectsState>(EMPTY_STATE);

  const refresh = useCallback(async () => {
    setState((prev) => ({ ...prev, loading: true, error: null }));
    try {
      const projects = await listProjects();
      const paths = projects.map((project) => project.path);
      const statuses = await gitProjectsStatus(paths.length ? paths : undefined);
      setState({
        loading: false,
        projects,
        statuses,
        error: null,
      });
    } catch (error) {
      setState((prev) => ({
        ...prev,
        loading: false,
        error: error instanceof Error ? error.message : String(error),
      }));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      setState((prev) => ({ ...prev, loading: true, error: null }));
      try {
        const projects = await listProjects();
        const paths = projects.map((project) => project.path);
        const statuses = await gitProjectsStatus(paths.length ? paths : undefined);
        if (cancelled) return;
        setState({ loading: false, projects, statuses, error: null });
      } catch (error) {
        if (cancelled) return;
        setState((prev) => ({
          ...prev,
          loading: false,
          error: error instanceof Error ? error.message : String(error),
        }));
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, []);

  return {
    ...state,
    refresh,
  };
}
