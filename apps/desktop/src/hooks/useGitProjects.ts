import { useCallback, useEffect, useState } from "react";
import { gitProjectsStatus, listProjects } from "../api";
import type { GitProjectStatus, Project } from "../types";

interface GitProjectsState {
  loading: boolean;
  refreshing: boolean;
  projects: Project[];
  statuses: GitProjectStatus[];
  error: string | null;
  cachedAt: number | null;
}

const EMPTY_STATE: GitProjectsState = {
  loading: false,
  refreshing: false,
  projects: [],
  statuses: [],
  error: null,
  cachedAt: null,
};

type GitProjectsCache = {
  version: 1;
  cachedAt: number;
  projects: Project[];
  statuses: GitProjectStatus[];
};

const GIT_PROJECTS_CACHE_KEY = "deepagent:git-projects-cache";
const GIT_PROJECTS_CACHE_TTL_MS = 30_000;

let sharedState: GitProjectsState | null = null;
let inFlight: Promise<GitProjectsState> | null = null;
const listeners = new Set<(state: GitProjectsState) => void>();

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

function stateFromCache(cache: GitProjectsCache): GitProjectsState {
  return {
    loading: false,
    refreshing: false,
    projects: cache.projects,
    statuses: cache.statuses,
    error: null,
    cachedAt: cache.cachedAt,
  };
}

function getInitialState(): GitProjectsState {
  if (sharedState) return sharedState;
  const cached = readGitProjectsCache();
  if (!cached) return EMPTY_STATE;
  sharedState = stateFromCache(cached);
  return sharedState;
}

function publishState(state: GitProjectsState) {
  sharedState = state;
  listeners.forEach((listener) => listener(state));
}

function hasGitProjectsData(state: GitProjectsState): boolean {
  return state.projects.length > 0 || state.statuses.length > 0;
}

function isFresh(state: GitProjectsState): boolean {
  return Boolean(state.cachedAt && Date.now() - state.cachedAt < GIT_PROJECTS_CACHE_TTL_MS);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function fetchGitProjects(): Promise<GitProjectsState> {
  if (inFlight) return inFlight;
  inFlight = (async () => {
    const projects = await listProjects();
    const paths = projects.map((project) => project.path);
    const statuses = await gitProjectsStatus(paths.length ? paths : undefined);
    const cachedAt = Date.now();
    writeGitProjectsCache(projects, statuses);
    const nextState: GitProjectsState = {
      loading: false,
      refreshing: false,
      projects,
      statuses,
      error: null,
      cachedAt,
    };
    publishState(nextState);
    return nextState;
  })().finally(() => {
    inFlight = null;
  });
  return inFlight;
}

async function loadGitProjects(showBlockingLoading: boolean) {
  const current = sharedState ?? getInitialState();
  const hasData = hasGitProjectsData(current);
  publishState({
    ...current,
    loading: showBlockingLoading && !hasData,
    refreshing: hasData,
    error: null,
  });
  try {
    await fetchGitProjects();
  } catch (error) {
    const previous = sharedState ?? current;
    publishState({
      ...previous,
      loading: false,
      refreshing: false,
      error: errorMessage(error),
    });
  }
}

export function useGitProjects() {
  const [state, setState] = useState<GitProjectsState>(getInitialState);

  const refresh = useCallback(async () => {
    await loadGitProjects(true);
  }, []);

  useEffect(() => {
    listeners.add(setState);
    const current = sharedState ?? getInitialState();
    if (!hasGitProjectsData(current) || !isFresh(current)) {
      window.setTimeout(() => {
        void loadGitProjects(!hasGitProjectsData(current));
      }, 0);
    }
    return () => {
      listeners.delete(setState);
    };
  }, []);

  return {
    ...state,
    refresh,
  };
}
