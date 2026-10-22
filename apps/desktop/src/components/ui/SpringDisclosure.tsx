import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import gsap from "gsap";
import { SOFT_SPRING, prefersReducedMotion, springEase } from "./springMotion";
import { cn } from "../shadcn/utils";

/** Layout-size animation shared by process details, attachments and sidebars. */
export function SpringDisclosure({ open, children, className, axis = "height", size,
  unmountOnExit = false, animateOnMount = false,
}: { open: boolean; children: ReactNode; className?: string; axis?: "height" | "width";
  size?: number; unmountOnExit?: boolean; animateOnMount?: boolean }) {
  const shellRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const lastSize = useRef(0);
  const initialized = useRef(false);
  const tweenRef = useRef<gsap.core.Timeline>();
  const [present, setPresent] = useState(open);

  useLayoutEffect(() => {
    const first = !initialized.current;
    initialized.current = true;
    const shell = shellRef.current, body = bodyRef.current;
    if (!shell || !body) return;
    if (open) setPresent(true);
    body.inert = !open;
    let disposed = false;
    const animate = () => {
      const natural = axis === "width" ? size ?? body.offsetWidth : body.offsetHeight;
      const target = open ? natural : 0;
      if (shell.style[axis] === "auto" && !open) gsap.set(shell, { [axis]: lastSize.current });
      lastSize.current = natural || lastSize.current;
      tweenRef.current?.kill();
      const reduced = prefersReducedMotion() || (first && !animateOnMount);
      const m = SOFT_SPRING;
      tweenRef.current = gsap.timeline({ onComplete: () => {
        if (disposed) return;
        if (open && axis === "height") shell.style.height = "auto";
        if (!open && unmountOnExit) setPresent(false);
      } });
      tweenRef.current.to(shell, { [axis]: target, duration: reduced ? 0 : m.duration,
        ease: springEase(m.stiffness, m.damping, m.duration) }, 0);
      tweenRef.current.to(body, { opacity: open ? 1 : 0, y: open ? 0 : -3,
        filter: open ? "blur(0px)" : "blur(2px)", duration: reduced ? 0 : open ? 0.24 : 0.14,
        ease: "power2.out" }, 0);
    };
    animate();
    const observer = new ResizeObserver(() => {
      const next = axis === "width" ? size ?? body.offsetWidth : body.offsetHeight;
      if (open && Math.abs(next - lastSize.current) > 0.5) animate();
    });
    observer.observe(body);
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    media.addEventListener("change", animate);
    return () => { disposed = true; observer.disconnect(); media.removeEventListener("change", animate); tweenRef.current?.kill(); };
  }, [open, axis, size, unmountOnExit, animateOnMount]);

  if (unmountOnExit && !open && !present) return null;
  return <div ref={shellRef} data-spring-disclosure={axis} aria-hidden={!open}
    className={cn("overflow-hidden", axis === "width" && "h-full shrink-0", className)} style={{ [axis]: 0 }}>
    <div ref={bodyRef} className={axis === "height" ? "flow-root" : "h-full"}
      style={axis === "width" ? { width: size } : undefined}>{children}</div>
  </div>;
}
