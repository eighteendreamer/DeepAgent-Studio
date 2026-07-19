import { useCallback, useEffect, useState } from "react";
import { DockviewReact, type DockviewReadyEvent } from "dockview-react";
import "dockview-react/dist/styles/dockview.css";
import { CanvasTitleBar } from "./CanvasTitleBar";

const LEGACY_CANVAS_LAYOUT_KEY = "deepagent:studio-canvas-layout:v1";
const EMPTY_COMPONENTS = {};

export function CanvasApp() {
  const [isDark, setIsDark] = useState(() => document.documentElement.classList.contains("dark"));

  useEffect(() => {
    const observer = new MutationObserver(() => {
      setIsDark(document.documentElement.classList.contains("dark"));
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  const handleReady = useCallback((event: DockviewReadyEvent) => {
    window.localStorage.removeItem(LEGACY_CANVAS_LAYOUT_KEY);
    event.api.clear();
  }, []);

  return (
    <div className="flex h-screen w-full flex-col overflow-hidden bg-white text-text-base">
      <CanvasTitleBar />
      <div className="studio-canvas-dockview min-h-0 flex-1">
        <DockviewReact
          className={isDark ? "dockview-theme-dark" : "dockview-theme-light"}
          components={EMPTY_COMPONENTS}
          onReady={handleReady}
        />
      </div>
    </div>
  );
}
