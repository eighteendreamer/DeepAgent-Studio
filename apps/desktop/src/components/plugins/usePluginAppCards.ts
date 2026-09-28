import { useCallback, useEffect, useState } from "react";
import { listPluginApps, PLUGINS_CHANGED_EVENT } from "../../api";
import { pluginAppToToolCard } from "./pluginRegistry";
import type { PluginToolCard } from "./pluginTypes";

/** Launcher entries come only from enabled plugins with renderable host apps. */
export function usePluginAppCards(active: boolean) {
  const [cards, setCards] = useState<PluginToolCard[]>([]);

  const refresh = useCallback(async () => {
    try {
      const apps = await listPluginApps();
      const next = apps.map(pluginAppToToolCard).filter((card): card is PluginToolCard => card !== null);
      setCards(next);
      return next;
    } catch (error) {
      console.warn("failed to load plugin apps", error);
      setCards([]);
      return [];
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    const onPluginsChanged = () => void refresh();
    window.addEventListener(PLUGINS_CHANGED_EVENT, onPluginsChanged);
    return () => window.removeEventListener(PLUGINS_CHANGED_EVENT, onPluginsChanged);
  }, [active, refresh]);

  return [cards, refresh] as const;
}
