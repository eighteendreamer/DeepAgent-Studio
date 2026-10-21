import { useCallback, useEffect, useRef, type ForwardedRef } from "react";
import gsap from "gsap";
import { prefersReducedMotion, springEase } from "./springMotion";

/** Pointer press/release only: never animate a hover or move an icon separately. */
export function useSpringPress<T extends HTMLElement>(forwardedRef?: ForwardedRef<T>) {
  const local = useRef<T | null>(null);
  const ref = useCallback((node: T | null) => {
    local.current = node;
    if (typeof forwardedRef === "function") forwardedRef(node);
    else if (forwardedRef) forwardedRef.current = node;
  }, [forwardedRef]);
  useEffect(() => {
    const node = local.current;
    if (!node) return;
    let pressed = false;
    let tween: gsap.core.Tween | undefined;
    const release = () => {
      if (!pressed) return;
      pressed = false;
      tween?.kill();
      tween = gsap.to(node, { scale: 1, duration: prefersReducedMotion() ? 0 : 0.38,
        ease: springEase(420, 24, 0.38) });
    };
    const down = () => {
      if (node.matches(":disabled") || prefersReducedMotion()) return;
      pressed = true; tween?.kill();
      tween = gsap.to(node, { scale: 0.97, duration: 0.075, ease: "power2.out" });
    };
    node.addEventListener("pointerdown", down);
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
    window.addEventListener("blur", release);
    return () => {
      node.removeEventListener("pointerdown", down);
      window.removeEventListener("pointerup", release);
      window.removeEventListener("pointercancel", release);
      window.removeEventListener("blur", release);
      tween?.kill(); gsap.set(node, { clearProps: "scale" });
    };
  }, []);
  return ref;
}
