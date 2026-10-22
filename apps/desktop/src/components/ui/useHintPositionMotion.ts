import { useLayoutEffect, useState } from "react";
import gsap from "gsap";
import { prefersReducedMotion } from "./springMotion";

/** Keep the popup mounted; smooth the positioner's anchor changes on a separate
 * wrapper so Base UI positioning and the hint's entrance transform stay intact. */
export function useHintPositionMotion(enabled: boolean) {
  const [positioner, positionerRef] = useState<HTMLDivElement | null>(null);
  const [shift, shiftRef] = useState<HTMLDivElement | null>(null);
  useLayoutEffect(() => {
    if (!enabled || !positioner || !shift) return;
    let previous: DOMRect | null = null;
    let tween: gsap.core.Tween | undefined;
    let frame = 0;
    let disposed = false;
    const remember = () => {
      if (disposed) return;
      previous = positioner.getBoundingClientRect();
    };
    // Initial anchor positioning happens after this child's layout effect.
    frame = requestAnimationFrame(() => { frame = requestAnimationFrame(remember); });
    const observer = new MutationObserver(() => {
      const next = positioner.getBoundingClientRect();
      if (previous && (Math.abs(next.left - previous.left) > .5 || Math.abs(next.top - previous.top) > .5)) {
        const x = previous.left - next.left + Number(gsap.getProperty(shift, "x"));
        const y = previous.top - next.top + Number(gsap.getProperty(shift, "y"));
        tween?.kill();
        gsap.set(shift, { x, y });
        tween = gsap.to(shift, { x: 0, y: 0, duration: prefersReducedMotion() ? 0 : .16, ease: "power2.out" });
      }
      if (previous) previous = next;
    });
    observer.observe(positioner, { attributes: true, attributeFilter: ["style"] });
    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      observer.disconnect(); tween?.kill();
      gsap.set(shift, { clearProps: "transform" });
    };
  }, [enabled, positioner, shift]);
  return { positionerRef, shiftRef };
}
