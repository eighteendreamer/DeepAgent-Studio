export interface GitUiSettings {
  branchPrefix: string;
  confirmBeforePush: boolean;
  batchStageAll: boolean;
  commitInstructions: string;
}

const STORAGE_KEY = "deepagent.git.settings.v1";

export const DEFAULT_GIT_UI_SETTINGS: GitUiSettings = {
  branchPrefix: "codex/",
  confirmBeforePush: true,
  batchStageAll: false,
  commitInstructions: "",
};

export function getGitUiSettings(): GitUiSettings {
  if (typeof window === "undefined") return DEFAULT_GIT_UI_SETTINGS;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_GIT_UI_SETTINGS;
    const parsed = JSON.parse(raw) as Partial<GitUiSettings>;
    return {
      branchPrefix: normalizeBranchPrefix(parsed.branchPrefix),
      confirmBeforePush: parsed.confirmBeforePush ?? DEFAULT_GIT_UI_SETTINGS.confirmBeforePush,
      batchStageAll: parsed.batchStageAll ?? DEFAULT_GIT_UI_SETTINGS.batchStageAll,
      commitInstructions: parsed.commitInstructions ?? DEFAULT_GIT_UI_SETTINGS.commitInstructions,
    };
  } catch {
    return DEFAULT_GIT_UI_SETTINGS;
  }
}

export function saveGitUiSettings(settings: GitUiSettings): GitUiSettings {
  const next: GitUiSettings = {
    ...settings,
    branchPrefix: normalizeBranchPrefix(settings.branchPrefix),
  };
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  return next;
}

export function normalizeBranchPrefix(value: string | undefined): string {
  const trimmed = (value ?? DEFAULT_GIT_UI_SETTINGS.branchPrefix).trim();
  if (!trimmed) return "";
  return trimmed.endsWith("/") ? trimmed : `${trimmed}/`;
}
