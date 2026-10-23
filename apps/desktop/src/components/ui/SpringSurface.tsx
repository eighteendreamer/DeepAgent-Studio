import {
  createContext, forwardRef, useCallback, useContext, useImperativeHandle,
  useLayoutEffect, useMemo, useRef, type HTMLAttributes, type Ref,
} from "react";
import gsap from "gsap";
import { continuousRadius, computedCornerCurvature } from "../../theme/continuousCorners";
import { SHARE_MOTION, motionColor, prefersReducedMotion, springEase } from "./springMotion";
import "./menuSurface.css";

type PopupActions = { unmount: () => void; close: () => void };
type PopupChange = { isCanceled: boolean; trigger?: Element | null; preventUnmountOnClose: () => void };
type MotionContext = { trigger: { current: HTMLElement | null }; onExited: () => void };
export const SpringPopupContext = createContext<MotionContext | null>(null);

// Base UI's documented external-animation lifecycle; business open state closes immediately.
export function usePopupMotionRoot(externalActions?: Ref<PopupActions | null>) {
  const actionsRef = useRef<PopupActions | null>(null);
  const trigger = useRef<HTMLElement | null>(null);
  useImperativeHandle(externalActions, () => ({
    unmount: () => actionsRef.current?.unmount(), close: () => actionsRef.current?.close(),
  }), []);
  const context = useMemo(() => ({ trigger, onExited: () => actionsRef.current?.unmount() }), []);
  const onChange = (open: boolean, details: PopupChange) => {
    if (details.isCanceled) return;
    if (open && details.trigger instanceof HTMLElement) trigger.current = details.trigger;
    if (!open && !prefersReducedMotion()) details.preventUnmountOnClose();
  };
  return { actionsRef, context, onChange };
}

type Props = HTMLAttributes<HTMLDivElement> & {
  motionTone?: "menu" | "hint";
  anchor?: { current: HTMLElement | null };
  openingPointer?: { current: boolean };
  onMotionExit?: () => void;
};

/** Keep library positioning/focus intact; morph one visible shell from its trigger. */
export const SpringSurface = forwardRef<HTMLDivElement, Props>(function SpringSurface(
  { children, className, motionTone = "menu", anchor, openingPointer, onMotionExit, ...props }, forwardedRef,
) {
  const nodeRef = useRef<HTMLDivElement | null>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const compactRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const popup = useContext(SpringPopupContext);
  const latest = useRef({ anchor, openingPointer, popup, onMotionExit });
  latest.current = { anchor, openingPointer, popup, onMotionExit };
  const setRef = useCallback((node: HTMLDivElement | null) => {
    nodeRef.current = node;
    if (typeof forwardedRef === "function") forwardedRef(node);
    else if (forwardedRef) forwardedRef.current = node;
  }, [forwardedRef]);

  useLayoutEffect(() => {
    const node = nodeRef.current, surface = surfaceRef.current, compact = compactRef.current, body = bodyRef.current;
    if (!node || !surface || !compact || !body) return;
    let timeline: gsap.core.Timeline | undefined;
    let resize: gsap.core.Tween | undefined;
    let previousOpen: boolean | undefined;
    let disposed = false;
    const targetWidth = node.offsetWidth;
    let targetHeight = node.offsetHeight;
    const css = getComputedStyle(node);
    const paddingX = parseFloat(css.paddingLeft) + parseFloat(css.paddingRight);
    const paddingY = parseFloat(css.paddingTop) + parseFloat(css.paddingBottom);
    const borderX = parseFloat(css.borderLeftWidth) + parseFloat(css.borderRightWidth);
    const borderY = parseFloat(css.borderTopWidth) + parseFloat(css.borderBottomWidth);
    const borderLeft = parseFloat(css.borderLeftWidth);
    const borderTop = parseFloat(css.borderTopWidth);
    const radius = parseFloat(css.borderTopLeftRadius) || continuousRadius(16);
    const curvature = computedCornerCurvature(css);
    const maxHeight = parseFloat(css.maxHeight);
    const context = gsap.context(() => {}, node);
    const trigger = (latest.current.anchor ?? latest.current.popup?.trigger)?.current;
    const triggerStyle = trigger?.style.opacity ?? "";
    let ownsTriggerOpacity = false;
    const triggerCss = trigger ? getComputedStyle(trigger) : null;
    const buttonRadius = parseFloat(triggerCss?.borderTopLeftRadius ?? "0");
    const buttonCurvature = triggerCss ? computedCornerCurvature(triggerCss) : curvature;
    const buttonBackground = motionColor(triggerCss?.backgroundColor ?? "transparent");
    const initial = node.getBoundingClientRect();
    const origin = trigger?.getBoundingClientRect();
    const side = node.getAttribute("data-side") ?? (origin && initial.bottom <= origin.top + 12 ? "top" : origin && initial.left >= origin.right ? "right" : "bottom");
    const align = node.getAttribute("data-align") ?? (origin && Math.abs(initial.right - origin.right) < Math.abs(initial.left - origin.left) ? "end" : "start");
    const sideways = side === "left" || side === "right";
    const background = motionColor(css.backgroundColor), shadow = css.boxShadow, borderColor = motionColor(css.borderTopColor);
    const naturalOverflowX = css.overflowX, naturalOverflowY = css.overflowY;
    const restoreTrigger = () => {
      if (trigger && ownsTriggerOpacity) trigger.style.opacity = triggerStyle;
      ownsTriggerOpacity = false;
    };
    context.add(() => gsap.set(surface, { visibility: "visible" }));
    // The library's outer box remains at its natural size, so collision detection
    // never chases the animated dimensions. Only this inner shell is visible.
    if (motionTone === "menu") context.add(() => {
      gsap.set(node, { position: "relative", width: targetWidth, height: targetHeight, backgroundColor: "transparent",
        borderColor: "transparent", boxShadow: "none", overflow: "visible", pointerEvents: "none" });
      gsap.set(surface, { position: "absolute", boxSizing: "border-box", width: targetWidth, height: targetHeight,
        padding: css.padding, borderWidth: css.borderWidth, borderStyle: css.borderStyle,
        borderColor, borderRadius: radius, "--spring-corner-curvature": curvature, backgroundColor: background, boxShadow: shadow });
      body.style.width = `${Math.max(0, targetWidth - paddingX - borderX)}px`;
      if (!sideways && trigger && origin && origin.width > 1 && origin.height > 1) {
        const face = trigger.cloneNode(true) as HTMLElement;
        face.removeAttribute("id");
        for (const element of face.querySelectorAll("[id], [autofocus]")) {
          element.removeAttribute("id"); element.removeAttribute("autofocus");
        }
        Object.assign(face.style, { width: `${origin.width}px`, height: `${origin.height}px`,
          margin: "0", maxWidth: "none", opacity: triggerCss?.opacity ?? "1", transform: "none" });
        compact.replaceChildren(face);
        compact.inert = true;
      }
    });
    const dimensions = () => {
      const rect = trigger?.getBoundingClientRect();
      const heightLimit = rect && side === "top" ? rect.bottom - 16
        : rect && side === "bottom" ? window.innerHeight - rect.top - 16 : window.innerHeight - 32;
      return { width: Math.min(targetWidth, window.innerWidth - 32), height: Math.min(targetHeight, Math.max(24, heightLimit)) };
    };
    const settleOverflow = () => gsap.set(surface, { overflowX: naturalOverflowX,
      overflowY: dimensions().height < targetHeight ? "auto" : naturalOverflowY });
    const place = () => {
      if (motionTone !== "menu" || disposed) return;
      const base = node.getBoundingClientRect(), rect = trigger?.getBoundingClientRect();
      const width = surface.offsetWidth, height = surface.offsetHeight;
      let left = base.left, top = base.top;
      if (rect) {
        if (side === "top" || side === "bottom") {
          left = align === "end" ? rect.right - width : align === "center" ? rect.left + (rect.width - width) / 2 : rect.left;
          top = side === "top" ? rect.bottom - height : rect.top;
        } else {
          // Navigation and cascading menus keep their original side placement.
          left = base.left + (side === "left" ? node.offsetWidth - width : 0);
          top = base.top;
        }
        left = Math.max(8, Math.min(left, window.innerWidth - width - 8));
        top = Math.max(8, Math.min(top, window.innerHeight - height - 8));
        compact.style.left = `${rect.left - left - borderLeft}px`;
        compact.style.top = `${rect.top - top - borderTop}px`;
      }
      surface.style.left = `${left - base.left - borderLeft}px`;
      surface.style.top = `${top - base.top - borderTop}px`;
    };
    const isOpen = () => node.getAttribute("data-state") === "open" || node.hasAttribute("data-open");
    let faceReady = false, pointerHeld = false;
    let releaseFrame = 0;
    const enablePointer = () => {
      if (motionTone === "menu") body.style.pointerEvents = isOpen() && faceReady && !pointerHeld ? "auto" : "none";
    };
    const releasePointer = () => {
      cancelAnimationFrame(releaseFrame);
      releaseFrame = requestAnimationFrame(() => {
        pointerHeld = false;
        if (latest.current.openingPointer) latest.current.openingPointer.current = false;
        enablePointer();
      });
    };
    document.addEventListener("pointerup", releasePointer, true);
    document.addEventListener("pointercancel", releasePointer, true);
    const finishExit = () => {
      if (!disposed && !isOpen()) {
        restoreTrigger();
        surface.style.visibility = "hidden";
        gsap.set(node, { autoAlpha: 0 });
        (latest.current.onMotionExit ?? latest.current.popup?.onExited)?.();
      }
    };
    const play = () => context.add(() => {
      const open = isOpen();
      if (open === previousOpen) return;
      const first = previousOpen === undefined;
      previousOpen = open;
      timeline?.kill(); resize?.kill();
      node.inert = !open;
      faceReady = false;
      pointerHeld = open && !!latest.current.openingPointer?.current;
      enablePointer();
      // A primitive can remount its closed popup during the final unmount commit.
      // That instance has no open face to fold; never paint its natural size.
      if (first && !open) {
        restoreTrigger();
        gsap.set(node, { autoAlpha: 0 });
        gsap.set(surface, { visibility: "hidden" });
        return;
      }
      if (motionTone === "hint") {
        if (prefersReducedMotion()) {
          gsap.set(node, { autoAlpha: open ? 1 : 0 });
          if (!open) finishExit();
          return;
        }
        if (open) gsap.set(node, { opacity: 0, scale: 0.98, y: 2 });
        timeline = gsap.timeline({ onComplete: open ? undefined : finishExit });
        timeline.to(node, { autoAlpha: open ? 1 : 0, scale: open ? 1 : 0.98,
          y: open ? 0 : 2, duration: open ? 0.18 : 0.12, ease: "power2.out" });
        return;
      }
      const triggerRect = trigger?.getBoundingClientRect();
      const fromWidth = sideways ? 4 : triggerRect?.width ?? targetWidth * 0.7;
      const fromHeight = triggerRect?.height ?? 32;
      const expanded = dimensions();
      const motion = open ? SHARE_MOTION.open : SHARE_MOTION.close;
      if (trigger && !sideways) { trigger.style.opacity = "0"; ownsTriggerOpacity = true; }
      gsap.set(node, { visibility: "visible", opacity: 1 });
      gsap.set(surface, { visibility: "visible", overflow: "hidden", pointerEvents: open ? "auto" : "none" });
      if (prefersReducedMotion()) {
        gsap.set(surface, { width: open ? expanded.width : fromWidth, height: open ? expanded.height : fromHeight,
          borderRadius: open ? radius : buttonRadius, "--spring-corner-curvature": open ? curvature : buttonCurvature,
          backgroundColor: open ? background : buttonBackground,
          boxShadow: open ? shadow : "none", borderColor: open ? borderColor : "transparent" });
        gsap.set(compact, { opacity: open ? 0 : 1 });
        gsap.set(body, { opacity: open ? 1 : 0, filter: "blur(0px)" });
        place();
        faceReady = open; enablePointer();
        if (!open) finishExit();
        else settleOverflow();
        return;
      }
      if (open && timeline === undefined) {
        gsap.set(surface, { width: fromWidth, height: fromHeight, borderRadius: buttonRadius,
          "--spring-corner-curvature": buttonCurvature,
          backgroundColor: buttonBackground, boxShadow: "none", borderColor: "transparent" });
        gsap.set(compact, { opacity: 1 });
        gsap.set(body, { opacity: 0, filter: "blur(3px)" });
      }
      timeline = gsap.timeline({ onComplete: () => {
        if (!open) { finishExit(); return; }
        settleOverflow();
      } });
      timeline.to(surface, { width: open ? expanded.width : fromWidth,
        duration: motion.width.duration, ease: springEase(motion.width.stiffness, motion.width.damping, motion.width.duration) }, motion.width.delay);
      timeline.to(surface, { height: open ? expanded.height : fromHeight,
        duration: motion.height.duration, ease: springEase(motion.height.stiffness, motion.height.damping, motion.height.duration) }, motion.height.delay);
      timeline.to(surface, { borderRadius: open ? radius : buttonRadius,
        "--spring-corner-curvature": open ? curvature : buttonCurvature,
        backgroundColor: open ? background : buttonBackground, boxShadow: open ? shadow : "none",
        borderColor: open ? borderColor : "transparent", duration: 0.22, ease: "power2.out" }, 0);
      timeline.to(compact, { opacity: open ? 0 : 1, duration: 0.14, ease: "power2.out" }, open ? 0 : 0.2);
      timeline.to(body, { opacity: open ? 1 : 0,
        filter: open ? "blur(0px)" : "blur(3px)", duration: open ? 0.25 : 0.14, ease: "power2.out" }, open ? 0.12 : 0);
      if (open) timeline.call(() => { faceReady = true; enablePointer(); }, [], 0.37);
      if (open) {
        const rows = Array.from(body.querySelectorAll<HTMLElement>("[role=menuitem], [role=menuitemradio], [data-menu-item]")).slice(0, 24);
        timeline.fromTo(rows, { opacity: 0, y: 4 }, { opacity: 1, y: 0,
          duration: 0.25, stagger: Math.min(0.018, 0.16 / Math.max(1, rows.length)), ease: "power2.out" }, 0.14);
      }
    });
    play(); place();
    if (motionTone === "menu") gsap.ticker.add(place);
    // Floating primitives position their parent after this child's layout effect.
    // Re-anchor before paint when that transform arrives, including production
    // builds where StrictMode does not replay layout effects.
    const positionObserver = new MutationObserver(place);
    if (motionTone === "menu" && node.parentElement) positionObserver.observe(node.parentElement,
      { attributes: true, attributeFilter: ["style"] });
    const observer = new MutationObserver(play);
    observer.observe(node, { attributes: true, attributeFilter: ["data-state", "data-open", "data-closed"] });
    let rendered = dimensions();
    const retarget = () => {
      if (!isOpen() || motionTone === "hint" || disposed) return;
      const width = dimensions().width;
      body.style.width = `${Math.max(0, width - paddingX - borderX)}px`;
      const contentHeight = body.offsetHeight + paddingY + borderY;
      const nextHeight = Number.isFinite(maxHeight) ? Math.min(contentHeight, maxHeight) : contentHeight;
      const contentChanged = Math.abs(nextHeight - targetHeight) >= 1;
      targetHeight = nextHeight;
      const next = dimensions();
      const widthChanged = Math.abs(next.width - rendered.width) >= 1;
      if (!contentChanged && !widthChanged && Math.abs(next.height - rendered.height) < 1) return;
      rendered = next;
      context.add(() => {
        resize?.kill(); gsap.killTweensOf(surface, widthChanged ? "width,height" : "height");
        gsap.set(node, { width, height: targetHeight });
        const m = SHARE_MOTION.resize;
        resize = gsap.to(surface, { ...(widthChanged ? {width} : {}), height: next.height, duration: prefersReducedMotion() ? 0 : m.duration,
          ease: springEase(m.stiffness, m.damping, m.duration),
          onComplete: settleOverflow,
        });
      });
    };
    const contentResize = new ResizeObserver(retarget);
    contentResize.observe(body);
    window.addEventListener("resize", retarget);
    return () => {
      disposed = true;
      observer.disconnect(); contentResize.disconnect(); positionObserver.disconnect();
      gsap.ticker.remove(place);
      window.removeEventListener("resize", retarget);
      document.removeEventListener("pointerup", releasePointer, true);
      document.removeEventListener("pointercancel", releasePointer, true);
      cancelAnimationFrame(releaseFrame);
      restoreTrigger(); compact.replaceChildren();
      timeline?.kill(); resize?.kill();
      context.revert();
      node.style.visibility = "hidden";
      surface.style.visibility = "hidden";
    };
  }, [motionTone]);

  return <div {...props} ref={setRef} data-spring-tone={motionTone} className={`spring-surface ui-menu-surface ${className ?? ""}`}>
    <div ref={surfaceRef} data-spring-shell>
      <div ref={compactRef} aria-hidden="true" className="pointer-events-none absolute" />
      <div ref={bodyRef} className="spring-surface-content">{children}</div>
    </div>
  </div>;
});
