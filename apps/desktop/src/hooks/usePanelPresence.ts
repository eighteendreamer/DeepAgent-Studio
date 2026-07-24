import { useEffect, useRef, useState } from "react";

export type PanelPresencePhase = "opening" | "open" | "closing" | "closed";

export function usePanelPresence(open: boolean, durationMs = 240) {
  const [phase, setPhase] = useState<PanelPresencePhase>(open ? "open" : "closed");
  const closeTimerRef = useRef<number | null>(null);
  const openFrameRef = useRef<number | null>(null);

  useEffect(() => {
    if (closeTimerRef.current != null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
    if (openFrameRef.current != null) {
      window.cancelAnimationFrame(openFrameRef.current);
      openFrameRef.current = null;
    }

    if (open) {
      setPhase((current) => (current === "closed" ? "opening" : "open"));
      openFrameRef.current = window.requestAnimationFrame(() => {
        openFrameRef.current = null;
        setPhase("open");
      });
      return;
    }

    setPhase((current) => (current === "closed" ? "closed" : "closing"));
    closeTimerRef.current = window.setTimeout(() => {
      closeTimerRef.current = null;
      setPhase("closed");
    }, durationMs);

    return () => {
      if (closeTimerRef.current != null) {
        window.clearTimeout(closeTimerRef.current);
        closeTimerRef.current = null;
      }
      if (openFrameRef.current != null) {
        window.cancelAnimationFrame(openFrameRef.current);
        openFrameRef.current = null;
      }
    };
  }, [durationMs, open]);

  return {
    phase,
    shouldRender: phase !== "closed",
    isVisible: phase === "open",
    isClosing: phase === "closing",
  };
}
