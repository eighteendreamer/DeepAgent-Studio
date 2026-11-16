import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import gsap from "gsap";
import { continuousRadius, computedCornerCurvature, CORNER_CURVATURE } from "../../theme/continuousCorners";
import { cn } from "../shadcn/utils";
import { MenuTriggerOpenContext, MorphPanelLayoutContext } from "./MorphPanelLayoutContext";
import { SHARE_MOTION, motionColor, prefersReducedMotion, springEase } from "./springMotion";
import "./menuSurface.css";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  layoutId: string;
  trigger: ReactNode;
  className?: string;
  panelClassName?: string;
  panelShadow?: string;
  panelAlign?: "left" | "right";
  panelPlacement?: "above" | "below";
  zIndex?: number;
  staggerContent?: boolean;
  unstyled?: boolean;
  triggerLayout?: "inline" | "block";
  children: ReactNode;
};

const OPEN_SHADOW = "0 6px 24px rgba(0,0,0,0.10)";
const CLOSED_SHADOW = "0 0 0 rgba(0,0,0,0)";

/** The approved share demo's single shell, compact face and expanded face. */
export function MorphingMenuShell({ open, onOpenChange, layoutId, trigger, className,
  panelClassName, panelShadow = OPEN_SHADOW, panelAlign = "left", panelPlacement = "above", zIndex = 50,
  staggerContent = false, unstyled = false, triggerLayout = "inline", children,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const anchorRef = useRef<HTMLDivElement>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const compactRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const previousOpen = useRef<boolean>();
  const latest = useRef({ open, onOpenChange });
  latest.current = { open, onOpenChange };
  const [present, setPresent] = useState(open);
  const [layoutAnimating, setLayoutAnimating] = useState(false);
  const showPanel = open || present;

  useEffect(() => {
    if (!open) return;
    const outside = (event: MouseEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) latest.current.onOpenChange(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") latest.current.onOpenChange(false);
    };
    document.addEventListener("mousedown", outside);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", outside);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);

  useLayoutEffect(() => {
    const shell = shellRef.current, anchor = anchorRef.current, compact = compactRef.current;
    const panel = panelRef.current, content = contentRef.current;
    if (!shell || !anchor || !compact) return;
    const first = previousOpen.current === undefined;
    previousOpen.current = open;
    let disposed = false;
    let timeline: gsap.core.Timeline | undefined;
    let resize: gsap.core.Tween | undefined;
    let restoreFocus = shell.contains(document.activeElement);

    const compactSize = () => {
      if (triggerLayout === "block") compact.style.width = `${anchor.clientWidth}px`;
      const button = compact.querySelector("button");
      const css = getComputedStyle(button ?? compact);
      const width = compact.offsetWidth, height = compact.offsetHeight;
      return { width, height, radius: Math.min(parseFloat(css.borderTopLeftRadius) || height / 2, height / 2),
        curvature: computedCornerCurvature(css),
        background: motionColor(css.backgroundColor) };
    };
    let closed = compactSize();
    const restClosed = () => {
      gsap.set(compact, { opacity: 1, clearProps: "position,left,right,top,bottom,transform" });
      gsap.set(shell, { clearProps: "position,left,right,top,bottom,width,height,maxWidth,overflow,borderRadius,backgroundColor,boxShadow,transform,--morph-corner-curvature" });
      anchor.style.width = ""; anchor.style.height = "";
      if (triggerLayout === "block") compact.style.width = "100%";
    };
    compact.inert = open;
    if (panel) panel.inert = !open;
    if (!showPanel || !panel || !content) {
      restClosed();
      return;
    }
    if (open) setPresent(true);

    // Reserve the compact control's layout slot. The same shell owns both sizes.
    anchor.style.width = triggerLayout === "block" ? "100%" : `${closed.width}px`;
    anchor.style.height = `${closed.height}px`;
    if (shell.style.position !== "absolute") {
      gsap.set(shell, { position: "absolute", width: closed.width, height: closed.height,
        maxWidth: "none", borderRadius: closed.radius, "--morph-corner-curvature": closed.curvature, backgroundColor: closed.background });
    }
    gsap.set(shell, { left: panelAlign === "left" ? 0 : "auto", right: panelAlign === "right" ? 0 : "auto",
      bottom: panelPlacement === "above" ? 0 : "auto", top: panelPlacement === "below" ? 0 : "auto" });
    gsap.set(compact, { position: "absolute", left: panelAlign === "left" ? 0 : "auto",
      right: panelAlign === "right" ? 0 : "auto", bottom: panelPlacement === "above" ? 0 : "auto",
      top: panelPlacement === "below" ? 0 : "auto" });
    const expandedSize = () => {
      panel.style.width = "";
      const scrollTop = content.scrollTop;
      content.style.maxHeight = "";
      content.style.overflowY = "";
      const width = Math.min(content.offsetWidth, window.innerWidth - 32);
      panel.style.width = `${width}px`;
      const rect = anchor.getBoundingClientRect();
      const availableHeight = panelPlacement === "above" ? rect.bottom - 16 : window.innerHeight - rect.top - 16;
      if (panel.offsetHeight > availableHeight) {
        content.style.maxHeight = `${Math.max(24, availableHeight)}px`;
        content.style.overflowY = "auto";
        content.scrollTop = scrollTop;
      }
      const left = panelAlign === "right" ? rect.right - width : rect.left;
      return { width, height: panel.offsetHeight,
        x: Math.max(16, Math.min(left, window.innerWidth - width - 16)) - left };
    };
    let expanded = expandedSize();
    const finish = () => {
      if (disposed || latest.current.open !== open) return;
      setLayoutAnimating(false);
      if (open) shell.style.overflow = "visible";
      else {
        restClosed();
        setPresent(false);
      }
    };
    const focus = () => {
      if (!disposed && restoreFocus && document.activeElement === document.body && latest.current.open === open) {
        (open ? panel : compact.querySelector<HTMLButtonElement>("button"))?.focus({ preventScroll: true });
      }
    };
    const play = () => {
      timeline?.kill(); resize?.kill();
      const reduced = prefersReducedMotion() || (first && open);
      if (reduced) {
        gsap.set(shell, { width: open ? expanded.width : closed.width, height: open ? expanded.height : closed.height,
          x: open ? expanded.x : 0, borderRadius: open ? continuousRadius(16) : closed.radius,
          "--morph-corner-curvature": open ? CORNER_CURVATURE : closed.curvature,
          backgroundColor: open ? motionColor(getComputedStyle(panel).backgroundColor) : closed.background,
          boxShadow: open ? panelShadow : CLOSED_SHADOW });
        gsap.set(compact, { opacity: open ? 0 : 1, scale: 1 });
        gsap.set(panel, { opacity: open ? 1 : 0, filter: "blur(0px)" });
        finish(); focus();
        return;
      }
      setLayoutAnimating(true);
      shell.style.overflow = "hidden";
      const motion = open ? SHARE_MOTION.open : SHARE_MOTION.close;
      timeline = gsap.timeline({ onComplete: finish });
      timeline.to(shell, { width: open ? expanded.width : closed.width, duration: motion.width.duration,
        ease: springEase(motion.width.stiffness, motion.width.damping, motion.width.duration) }, motion.width.delay);
      timeline.to(shell, { height: open ? expanded.height : closed.height, duration: motion.height.duration,
        ease: springEase(motion.height.stiffness, motion.height.damping, motion.height.duration) }, motion.height.delay);
      timeline.to(shell, { x: open ? expanded.x : 0, borderRadius: open ? continuousRadius(16) : closed.radius,
        "--morph-corner-curvature": open ? CORNER_CURVATURE : closed.curvature,
        backgroundColor: open ? motionColor(getComputedStyle(panel).backgroundColor) : closed.background,
        boxShadow: open ? panelShadow : CLOSED_SHADOW, duration: 0.22, ease: "power2.out" }, 0);
      timeline.to(compact, { opacity: open ? 0 : 1, duration: 0.14, ease: "power2.out" }, open ? 0 : 0.2);
      if (!open) timeline.fromTo(compact, { scale: 0.95 }, { scale: 1, duration: 0.48,
        ease: springEase(300, 21, 0.48) }, 0.2);
      timeline.to(panel, { opacity: open ? 1 : 0, filter: open ? "blur(0px)" : "blur(5px)",
        duration: open ? 0.24 : 0.18, ease: "power2.out" }, open ? 0.12 : 0);
      if (open && staggerContent) {
        const rows = Array.from(content.querySelectorAll("[role=menuitem], [data-menu-item]")).slice(0, 24);
        timeline.fromTo(rows, { opacity: 0, y: 6 }, { opacity: 1, y: 0,
          duration: 0.5, stagger: Math.min(0.025, 0.16 / Math.max(1, rows.length)), ease: springEase(280, 24, 0.5) }, 0.13);
      }
      timeline.call(focus, [], 0.36);
    };
    play();
    const retarget = () => {
      if (disposed) return;
      closed = compactSize();
      if (!latest.current.open) return;
      const next = expandedSize();
      if (Math.abs(next.width - expanded.width) < 0.5 && Math.abs(next.height - expanded.height) < 0.5 && Math.abs(next.x - expanded.x) < 0.5) return;
      const widthChanged = Math.abs(next.width - expanded.width) >= 0.5;
      expanded = next;
      resize?.kill();
      gsap.killTweensOf(shell, widthChanged ? "width,height,x" : "height,x");
      const motion = SHARE_MOTION.resize;
      resize = gsap.to(shell, { ...(widthChanged ? { width: expanded.width } : {}), x: expanded.x, height: expanded.height,
        duration: prefersReducedMotion() ? 0 : motion.duration,
        ease: springEase(motion.stiffness, motion.damping, motion.duration) });
    };
    const observer = new ResizeObserver(retarget);
    observer.observe(content); observer.observe(compact);
    const outsideFocus = (event: FocusEvent) => { if (!shell.contains(event.target as Node)) restoreFocus = false; };
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    document.addEventListener("focusin", outsideFocus);
    window.addEventListener("resize", retarget);
    media.addEventListener("change", play);
    return () => {
      disposed = true; timeline?.kill(); resize?.kill(); observer.disconnect();
      document.removeEventListener("focusin", outsideFocus);
      window.removeEventListener("resize", retarget);
      media.removeEventListener("change", play);
    };
  }, [open, showPanel, panelAlign, panelPlacement, panelShadow, triggerLayout]);

  return <div ref={containerRef} className={cn("relative", className)}>
    <div ref={anchorRef} className={cn("relative max-w-full align-top", triggerLayout === "block" ? "flex w-full" : "inline-flex")}>
      <div ref={shellRef} data-motion-id={layoutId} data-state={open ? "open" : "closed"}
        data-morph-placement={panelPlacement} className={cn("morph-menu-shell relative inline-flex max-w-full", triggerLayout === "block" && "w-full")}
        style={{ zIndex: showPanel ? zIndex : undefined, pointerEvents: !open && present ? "none" : undefined }}>
        <div ref={compactRef} className={cn("morph-menu-compact pointer-events-auto inline-flex max-w-full", triggerLayout === "block" ? "w-full" : "w-max")}
          aria-hidden={open}>
          <MenuTriggerOpenContext.Provider value={open}>{trigger}</MenuTriggerOpenContext.Provider>
        </div>
        {showPanel && <div ref={panelRef} data-morph-panel aria-hidden={!open} tabIndex={-1}
          className="morph-menu-expanded ui-menu-surface absolute left-0 top-0 w-max max-w-[calc(100vw-32px)] rounded-2xl bg-elevated-bg"
          style={{ opacity: 0, pointerEvents: open ? "auto" : "none" }}>
          <MorphPanelLayoutContext.Provider value={layoutAnimating}>
            <div ref={contentRef} className={cn("morph-menu-content max-w-[calc(100vw-32px)]", !unstyled && "bg-elevated-bg", panelClassName)}>{children}</div>
          </MorphPanelLayoutContext.Provider>
        </div>}
      </div>
    </div>
  </div>;
}
