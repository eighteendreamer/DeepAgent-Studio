import { convertFileSrc, invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

export interface Rect { x: number; y: number; width: number; height: number }
export interface PetAction { file: string; preview: string | null; width: number; height: number }
export interface PetState { actions: string[]; loop: boolean; fallback: string | null }
export interface Character {
  id: string;
  name: string;
  thumbnail: string;
  preview: string | null;
  canvas: { width: number; height: number };
  hitBox: Rect;
  defaultSize: number;
  actions: Record<string, PetAction>;
  states: Record<string, PetState>;
  groups: { id: string; actions: string[] }[];
}
export interface Preferences {
  enabled: boolean;
  sleeping: boolean;
  characterId: string;
  size: number;
  alwaysOnTop: boolean;
  followWork: boolean;
  bubbles: boolean;
}
export interface PetSnapshot {
  catalog: { characters: Character[] };
  resourceRoot: string;
  preferences: Preferences;
  stageTop: number;
  sizeMin: number;
  sizeMax: number;
  visible: boolean;
}
export interface WorkSnapshot { activeRuns: number; pendingApprovals: number; runningActions: number }
export type WorkState = "idle" | "thinking" | "working" | "waiting";
export type Completion = "completed" | "failed";

export const nativePet = isTauri();
export const getPet = () => invoke<PetSnapshot>("pet_get_state");
export const getPetPreferences = () => invoke<Preferences>("pet_get_preferences");
export const isPetAwake = (preferences?: Preferences) => Boolean(preferences?.enabled && !preferences.sleeping);
export const updatePet = (patch: Partial<Preferences>) => invoke<Preferences>("pet_update_preferences", { patch });
export const resetPet = () => invoke<void>("pet_reset_position");
export const petReady = () => invoke<void>("pet_ready");
export const petDrag = () => invoke<void>("pet_start_drag");
export const openMain = () => invoke<void>("pet_open_main");
export const getWork = () => invoke<WorkSnapshot>("pet_work_snapshot");
export const renderFailed = (error: string) => invoke<void>("pet_render_failed", { error });
export const setPetInteraction = (regions: Rect[], menuOpen: boolean) =>
  invoke<void>("pet_set_interaction", { interaction: { regions, menuOpen } });
export const petAsset = (root: string, relative: string) => convertFileSrc(`${root}/${relative}`);
export const listenPet = <T>(event: string, callback: (value: T) => void) =>
  getCurrentWebviewWindow().listen<T>(`pet://${event}`, ({ payload }) => callback(payload));

export function workState({ activeRuns, pendingApprovals, runningActions }: WorkSnapshot): WorkState {
  if (pendingApprovals) return "waiting";
  if (runningActions) return "working";
  return activeRuns ? "thinking" : "idle";
}

export function resolveAction(character: Character, requested: string, variant = 0, excluded?: string) {
  const seen = new Set<string>();
  let state: string | null = requested;
  while (state && !seen.has(state)) {
    seen.add(state);
    const entry: PetState | undefined = character.states[state];
    const available = entry?.actions.filter(name => name !== excluded && character.actions[name]);
    if (available?.length) {
      const name = available[variant % available.length];
      return { name, file: character.actions[name].file, loop: entry.loop };
    }
    state = entry?.fallback ?? (state === "idle" ? null : "idle");
  }
  return null;
}
