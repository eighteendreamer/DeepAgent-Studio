import { create } from "zustand";
import { getSidebarPreferences, updateSidebarPreferences } from "../api";
import type { SidebarPreferences, SidebarPreferenceAction } from "../types";

export const useSidebarPreferences = create<{ preferences: SidebarPreferences }>(() => ({
  preferences: { version: 1, revision: 0, sections: [], unread: {}, session_sections: {}, default_open_target: "VS Code" },
}));

let initialization: Promise<void> | undefined;
let queue = Promise.resolve();
let listening = false;
function applyPreferences(preferences: SidebarPreferences) {
  if (preferences.revision >= useSidebarPreferences.getState().preferences.revision) {
    useSidebarPreferences.setState({ preferences });
  }
}
export function loadSidebarPreferences() {
  if (!listening && "__TAURI_INTERNALS__" in window) {
    listening = true;
    void import("@tauri-apps/api/event").then((mod) => mod.listen<SidebarPreferences>("sidebar://preferences", (event) => applyPreferences(event.payload)))
      .catch((error) => { listening = false; console.warn("sidebar preference events unavailable", error); });
  }
  if (!initialization) initialization = getSidebarPreferences().then((preferences) => {
    applyPreferences(preferences);
  }).catch((error) => { initialization = undefined; throw error; });
  return initialization;
}

// Serialize user edits so late replies cannot replace newer sidebar state.
export function changeSidebarPreferences(action: SidebarPreferenceAction): Promise<void> {
  const task = queue.then(async () => {
    await loadSidebarPreferences();
    const preferences = await updateSidebarPreferences(action);
    applyPreferences(preferences);
  });
  queue = task.catch(() => {});
  return task;
}
