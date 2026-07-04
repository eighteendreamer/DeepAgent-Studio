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

type GitProjectsCache = {
  version: 1;
  cachedAt: number;
  projects: Project[];
  statuses: GitProjectStatus[];
};

const GIT_PROJECTS_CACHE_KEY = "deepagent:git-projects-cache";

function readGitProjectsCache(): GitProjectsCache | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(GIT_PROJECTS_CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<GitProjectsCache>;
    if (parsed.version !== 1 || !Array.isArray(parsed.projects) || !Array.isArray(parsed.statuses)) return null;
    return parsed as GitProjectsCache;
  } catch {
    return null;
  }
}

function writeGitProjectsCache(projects: Project[], statuses: GitProjectStatus[]) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(
      GIT_PROJECTS_CACHE_KEY,
      JSON.stringify({ version: 1, cachedAt: Date.now(), projects, statuses } satisfies GitProjectsCache),
    );
  } catch {
    // Best-effort UI cache.
  }
}

export function useGitProjects() {
  const [state, setState] = useState<GitProjectsState>(() => {
    const cached = readGitProjectsCache();
    if (!cached) return EMPTY_STATE;
    return {
      loading: false,
      projects: cached.projects,
      statuses: cached.statuses,
      error: null,
    };
  });

  const refresh = useCallback(async () => {
    setState((prev) => ({ ...prev, loading: true, error: null }));
    try {
      const projects = await listProjects();
      const paths = projects.map((project) => project.path);
      const statuses = await gitProjectsStatus(paths.length ? paths : undefined);
      writeGitProjectsCache(projects, statuses);
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
    if (readGitProjectsCache()) return;
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
