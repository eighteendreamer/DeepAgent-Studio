import { useEffect, useRef, useState } from "react";
import { getWebSearchSettings, setWebSearchSettings } from "../../api";
import type { WebSearchSettings } from "../../types";

type Edit = (current: WebSearchSettings) => WebSearchSettings;

/** Call-local UI edits only. SQLite/SettingsService remain the authority.
 * Like workflowPersistence, serialize publication, not just response rendering.
 * Rebase pending intents on the last successful receipt after a failed edit.
 */
export function useWebSearchSettings(onFailure: () => void) {
  const [settings, setSettings] = useState<WebSearchSettings | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const state = useRef({
    committed: null as WebSearchSettings | null,
    pending: [] as Edit[],
    tail: Promise.resolve(),
    mounted: true,
  });
  const publish = () => {
    const slot = state.current;
    if (slot.mounted && slot.committed) {
      setSettings(slot.pending.reduce((value, edit) => edit(value), slot.committed));
    }
  };
  const flush = async () => {
    while (true) {
      const tail = state.current.tail;
      await tail;
      if (tail === state.current.tail) return;
    }
  };
  const refresh = () => {
    const slot = state.current;
    slot.tail = slot.tail.then(async () => {
      try {
        slot.committed = await getWebSearchSettings();
        publish();
        if (slot.mounted) setLoadFailed(false);
      } catch {
        if (slot.mounted) setLoadFailed(true);
      }
    });
    return slot.tail;
  };
  useEffect(() => {
    state.current.mounted = true;
    void refresh();
    return () => { state.current.mounted = false; };
  }, []);

  const update = (edit: Edit) => {
    const slot = state.current;
    if (!slot.committed) return;
    slot.pending.push(edit);
    publish();
    slot.tail = slot.tail.then(async () => {
      try {
        slot.committed = await setWebSearchSettings(edit(slot.committed!));
      } catch {
        // Failed intent is removed, never replayed inside a later snapshot.
        if (slot.mounted) onFailure();
      } finally {
        slot.pending.shift();
        publish();
      }
    });
  };
  return { settings, loadFailed, update, flush, refresh };
}
