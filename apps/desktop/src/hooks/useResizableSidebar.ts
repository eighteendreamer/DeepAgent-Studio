import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Minimum width (px) the conversation/session column must always keep,
 * whether the right sidebar is being resized or the layout is just
 * reacting to a narrow window. Shared by `ChatView` and `StartView` so the
 * constraint is enforced identically in both places.
 */
export const MIN_CHAT_WIDTH = 500;

/** Default + floor width for the right sidebar itself. */
const DEFAULT_SIDEBAR_WIDTH = 360;
const MIN_SIDEBAR_WIDTH = 360;

/** Per-plugin minimum widths for the right workbench panel. */
export const SIDEBAR_MIN_WIDTH = {
  launcher: 280,
  chat: 420,
  files: 400,
  browser: 380,
  terminal: 360,
  default: 340,
} as const;

interface UseResizableSidebarOptions {
  /** Sidebar width floor. Defaults to 360px. */
  minWidth?: number;
  /** Initial (and default) sidebar width. Defaults to 360px. */
  defaultWidth?: number;
  /** Minimum width the conversation column must retain. Defaults to 500px. */
  minChatWidth?: number;
}

/**
 * Drag-to-resize state for the right sidebar, shared between `ChatView` and
 * `StartView`. Clamps the sidebar width so the conversation column never
 * drops below `minChatWidth` (500px), even mid-drag or after a window resize.
 */
export function useResizableSidebar(options: UseResizableSidebarOptions = {}) {
  const {
    minWidth = MIN_SIDEBAR_WIDTH,
    defaultWidth = DEFAULT_SIDEBAR_WIDTH,
    minChatWidth = MIN_CHAT_WIDTH,
  } = options;

  const [width, setWidthState] = useState(defaultWidth);
  const [isResizing, setIsResizing] = useState(false);
  const [isMaximized, setIsMaximized] = useState(false);
  const [boundaryElement, setBoundaryElement] = useState<HTMLElement | null>(null);
  const restoreWidthRef = useRef(defaultWidth);

  const sidebarRef = useCallback((node: HTMLElement | null) => {
    setBoundaryElement(node?.parentElement ?? null);
  }, []);

  const getBounds = useCallback(() => {
    const rect = boundaryElement?.getBoundingClientRect();
    if (rect && rect.width > 0) {
      return {
        right: rect.right,
        width: rect.width,
      };
    }
    return {
      right: window.innerWidth,
      width: window.innerWidth,
    };
  }, [boundaryElement]);

  const clampWidth = useCallback(
    (candidate: number) => {
      const maxWidth = Math.max(minWidth, getBounds().width - minChatWidth);
      return Math.min(Math.max(candidate, minWidth), maxWidth);
    },
    [getBounds, minChatWidth, minWidth],
  );

  const setWidth = useCallback(
    (candidate: number) => {
      setWidthState(clampWidth(candidate));
    },
    [clampWidth],
  );

  useEffect(() => {
    setWidthState((prev) => clampWidth(prev));
  }, [clampWidth]);

  // Re-clamp on window resize so a shrunk window can't leave the chat
  // column under `minChatWidth` even without the user dragging again.
  useEffect(() => {
    const onResize = () => setWidthState((prev) => clampWidth(prev));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [clampWidth]);

  useEffect(() => {
    if (!boundaryElement || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      setWidthState((prev) => clampWidth(prev));
    });
    observer.observe(boundaryElement);
    return () => observer.disconnect();
  }, [boundaryElement, clampWidth]);

  useEffect(() => {
    if (!isResizing) return;
    const handleMouseMove = (e: MouseEvent) => {
      setWidthState(clampWidth(getBounds().right - e.clientX));
    };
    const handleMouseUp = () => setIsResizing(false);

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [clampWidth, getBounds, isResizing]);

  const startResizing = useCallback(() => setIsResizing(true), []);

  const toggleMaximize = useCallback(() => {
    setIsMaximized((prev) => {
      if (prev) {
        setWidthState(clampWidth(restoreWidthRef.current));
        return false;
      }
      restoreWidthRef.current = width;
      return true;
    });
  }, [clampWidth, width]);

  const resetMaximize = useCallback(() => {
    setIsMaximized(false);
  }, []);

  return {
    width,
    setWidth,
    sidebarRef,
    isResizing,
    startResizing,
    isMaximized,
    toggleMaximize,
    resetMaximize,
  };
}
