import { useState, useEffect, useLayoutEffect, useRef, useCallback, useId, type ReactNode } from "react";
import { ArrowUp } from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import "./BottomBar.css";

// Keep hover transitions local: the mounted controls do not re-render as the
// pointer crosses the dock, and the canvas has no subscription to this state.
export function CanvasToolbarDock({ children }: { children: ReactNode }) {
  const [toolsOpen, setToolsOpen] = useState(false);
  const dockRef = useRef<HTMLDivElement>(null);
  const toolsRef = useRef<HTMLDivElement>(null);
  const rowRef = useRef<HTMLDivElement>(null);
  const launcherRef = useRef<HTMLButtonElement>(null);
  const pointerInside = useRef(false);
  const pointerDown = useRef(false);
  const keyboardInside = useRef(false);
  const restoringFocus = useRef(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout>>();
  const toolsId = useId();

  const openTools = useCallback(() => {
    clearTimeout(closeTimer.current);
    setToolsOpen(true);
  }, []);
  const scheduleClose = useCallback(() => {
    clearTimeout(closeTimer.current);
    if (pointerInside.current || pointerDown.current || keyboardInside.current) return;
    closeTimer.current = setTimeout(() => {
      if (!pointerInside.current && !pointerDown.current && !keyboardInside.current) {
        setToolsOpen(false);
      }
    }, 120);
  }, []);

  useLayoutEffect(() => {
    const dock = dockRef.current;
    const row = rowRef.current;
    const parent = dock?.parentElement;
    if (!dock || !row || !parent) return;
    const measure = () => {
      const width = Math.min(row.scrollWidth + 2, Math.max(44, parent.clientWidth - 32));
      dock.style.setProperty("--dock-width", `${width}px`);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(row);
    observer.observe(parent);
    return () => observer.disconnect();
  }, []);

  useLayoutEffect(() => {
    if (toolsRef.current) toolsRef.current.inert = !toolsOpen;
  }, [toolsOpen]);

  useEffect(() => {
    const release = () => {
      pointerDown.current = false;
      scheduleClose();
    };
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    return () => {
      clearTimeout(closeTimer.current);
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
    };
  }, [scheduleClose]);

  return (
    <div
      ref={dockRef}
      className="canvas-bottom-dock absolute bottom-4 left-1/2 -translate-x-1/2 z-[60]"
      data-expanded={toolsOpen}
      onPointerEnter={() => {
        pointerInside.current = true;
        openTools();
      }}
      onPointerLeave={() => {
        pointerInside.current = false;
        scheduleClose();
      }}
      onPointerDownCapture={() => { pointerDown.current = true; }}
      onFocusCapture={(event) => {
        if (!restoringFocus.current && event.target.matches(":focus-visible")) {
          keyboardInside.current = true;
          openTools();
        }
      }}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          keyboardInside.current = false;
          scheduleClose();
        }
      }}
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        clearTimeout(closeTimer.current);
        restoringFocus.current = true;
        launcherRef.current?.focus({ preventScroll: true });
        restoringFocus.current = false;
        keyboardInside.current = false;
        setToolsOpen(false);
      }}
    >
      <Button
        ref={launcherRef}
        variant="ghost"
        className="canvas-bottom-dock__launcher h-full w-full rounded-xl bg-transparent p-0 hover:bg-white/10"
        tabIndex={toolsOpen ? -1 : 0}
        aria-label="工具栏"
        aria-expanded={toolsOpen}
        aria-controls={toolsId}
        onClick={() => {
          keyboardInside.current = launcherRef.current?.matches(":focus-visible") ?? false;
          openTools();
        }}
      >
        <ArrowUp size={13} strokeWidth={1.8} color="rgba(248,248,248,0.7)" />
      </Button>
      <div ref={toolsRef} id={toolsId} className="canvas-bottom-dock__tools" role="toolbar" aria-label="画布工具" aria-hidden={!toolsOpen}>
          <div ref={rowRef} className="canvas-bottom-dock__row flex items-center gap-1 px-1.5">

            {children}
          </div>
      </div>
    </div>
  );
}
