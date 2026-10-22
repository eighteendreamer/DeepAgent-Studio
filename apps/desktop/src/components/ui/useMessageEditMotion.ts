import { useLayoutEffect, useRef } from "react";
import gsap from "gsap";
import { continuousRadius } from "../../theme/continuousCorners";
import { motionColor, prefersReducedMotion, SHARE_MOTION, springEase } from "./springMotion";

/** Approved E dimensions with A's model-menu timing. A single slot owns all
 * physical height changes; neither the answer nor the text is scaled. */
export function useMessageEditMotion(open: boolean, draft: string, contentVersion: unknown) {
  const slotRef = useRef<HTMLDivElement>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const compactRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const actionsRef = useRef<HTMLDivElement>(null);
  const editButtonRef = useRef<HTMLButtonElement>(null);
  const initialized = useRef(false);
  const previousOpen = useRef(false);
  const resizeRef = useRef<() => void>();

  useLayoutEffect(() => {
    const slot = slotRef.current, shell = shellRef.current, compact = compactRef.current;
    const editor = editorRef.current, textarea = textareaRef.current, actions = actionsRef.current;
    if (!slot || !shell || !compact || !editor || !textarea || !actions) return;
    const first = !initialized.current, entering = open && !previousOpen.current;
    const leaving = !open && previousOpen.current;
    const revealFromClosed = entering && Number(getComputedStyle(editor).opacity) < 0.01;
    initialized.current = true;
    previousOpen.current = open;
    let disposed = false;
    let timeline: gsap.core.Timeline | undefined, resizing: gsap.core.Timeline | undefined;
    let lastWidth = slot.clientWidth;
    const dimensions = () => {
      compact.style.maxWidth = `${slot.clientWidth * 0.8}px`;
      const closed = { width: compact.offsetWidth, height: compact.offsetHeight };
      const width = Math.min(slot.clientWidth, Math.max(Math.min(320, slot.clientWidth), slot.clientWidth * 0.65));
      editor.style.width = `${width}px`;
      textarea.style.height = "auto";
      textarea.style.height = `${Math.min(144, Math.max(48, Math.ceil(textarea.scrollHeight)))}px`;
      const height = editor.offsetHeight;
      return { closed, width, height, flow: height + 8 };
    };
    let size = dimensions();
    compact.inert = open;
    editor.inert = !open;
    actions.inert = open;
    if (first) {
      gsap.set(shell, { width: size.closed.width, height: size.closed.height,
        borderRadius: continuousRadius(16), borderTopRightRadius: continuousRadius(2), backgroundColor: "transparent" });
      gsap.set(slot, { height: size.closed.height + 28 });
      gsap.set(compact, { opacity: 1 });
      gsap.set(editor, { opacity: 0, filter: "blur(5px)" });
    }
    const finish = () => {
      if (disposed) return;
      shell.dataset.editAnimating = "false";
      if (!open) {
        // The original face paints the resting bubble, preserving send-flight
        // visibility, colors and padding on its existing registered target.
        gsap.set(shell, { backgroundColor: "transparent" });
        if (leaving) editButtonRef.current?.focus({ preventScroll: true });
      }
    };
    const play = (immediate = false) => {
      timeline?.kill(); resizing?.kill();
      const reduced = immediate || prefersReducedMotion();
      const d = (value: number) => reduced ? 0 : value;
      const m = open ? SHARE_MOTION.open : SHARE_MOTION.close;
      shell.dataset.editAnimating = "true";
      gsap.set(shell, { backgroundColor: motionColor(getComputedStyle(compact).backgroundColor) });
      timeline = gsap.timeline({ onComplete: finish });
      timeline.to(shell, { width: open ? size.width : size.closed.width,
        duration: d(m.width.duration), ease: springEase(m.width.stiffness, m.width.damping, m.width.duration) }, d(m.width.delay));
      timeline.to(shell, { height: open ? size.height : size.closed.height,
        duration: d(m.height.duration), ease: springEase(m.height.stiffness, m.height.damping, m.height.duration) }, d(m.height.delay));
      timeline.to(slot, { height: open ? size.flow : size.closed.height + 28,
        duration: d(m.height.duration), ease: springEase(m.height.stiffness, m.height.damping, m.height.duration) }, d(m.height.delay));
      timeline.to(shell, { borderRadius: continuousRadius(open ? 20 : 16), borderTopRightRadius: continuousRadius(open ? 20 : 2),
        boxShadow: open ? "0 6px 24px rgba(0,0,0,0.08)" : "0 0 0 rgba(0,0,0,0)", duration: d(0.22), ease: "power2.out" }, 0);
      timeline.to(compact, { opacity: open ? 0 : 1, duration: d(0.14), ease: "power2.out" }, d(open ? 0 : 0.2));
      timeline.to(actions, { opacity: open ? 0 : 1, duration: d(0.14), ease: "power2.out" }, d(open ? 0 : 0.2));
      timeline.to(editor, { opacity: open ? 1 : 0, filter: open ? "blur(0px)" : "blur(5px)",
        duration: d(open ? 0.24 : 0.18), ease: "power2.out" }, d(open ? 0.12 : 0));
      if (entering) {
        const controls = editor.querySelectorAll("[data-edit-action]");
        const reveal = { opacity: 1, y: 0, duration: d(0.5), stagger: reduced ? 0 : 0.025, ease: springEase(280, 24, 0.5) };
        if (revealFromClosed) timeline.fromTo(controls, { opacity: 0, y: 6 }, reveal, d(0.13));
        else timeline.to(controls, reveal, 0);
        timeline.call(() => {
          if (disposed) return;
          textarea.focus({ preventScroll: true });
          textarea.setSelectionRange(textarea.value.length, textarea.value.length);
        }, [], d(0.36));
      }
    };
    if (first && !open) finish(); else play();
    const resize = () => {
      const next = dimensions();
      if (open && Math.abs(next.width - size.width) < 0.5 && Math.abs(next.height - size.height) < 0.5) return;
      const widthChanged = Math.abs(next.width - size.width) >= 0.5;
      size = next;
      resizing?.kill();
      if (!open) {
        // Draft resets on cancel must never call this path while folding.
        gsap.set(shell, { width: next.closed.width, height: next.closed.height });
        gsap.set(slot, { height: next.closed.height + 28 });
        return;
      }
      gsap.killTweensOf(shell, widthChanged ? "width,height" : "height");
      gsap.killTweensOf(slot, "height");
      const m = SHARE_MOTION.resize;
      const transition = { duration: prefersReducedMotion() ? 0 : m.duration, ease: springEase(m.stiffness, m.damping, m.duration) };
      resizing = gsap.timeline();
      resizing.to(shell, { ...(widthChanged ? { width: next.width } : {}), height: next.height, ...transition }, 0);
      resizing.to(slot, { height: next.flow, ...transition }, 0);
    };
    resizeRef.current = resize;
    const observer = new ResizeObserver(() => {
      if (slot.clientWidth !== lastWidth) { lastWidth = slot.clientWidth; resize(); }
    });
    observer.observe(slot);
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onMotionChange = () => play(true);
    media.addEventListener("change", onMotionChange);
    return () => {
      disposed = true; observer.disconnect(); timeline?.kill(); resizing?.kill();
      media.removeEventListener("change", onMotionChange);
      resizeRef.current = undefined;
    };
  }, [open, contentVersion]);

  useLayoutEffect(() => {
    // A cancel also restores the draft. Resizing the closed face here would
    // snap it to its target before the return animation can run.
    if (open) resizeRef.current?.();
  }, [draft]);

  return { slotRef, shellRef, compactRef, editorRef, textareaRef, actionsRef, editButtonRef };
}
