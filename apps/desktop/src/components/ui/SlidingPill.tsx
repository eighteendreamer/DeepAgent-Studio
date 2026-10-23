import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type FocusEvent, type MouseEvent } from "react";
import gsap from "gsap";
import { cn } from "../shadcn/utils";
import { MOTION } from "./motion";

/**
 * 滑动指示器（静默着色药丸）—— 统一侧栏/导航的悬停滑动交互。
 *
 * 用法：
 *   const { containerRef, containerProps, indicatorStyle } = useSlidingIndicator({
 *     hoverSelector: "[data-nav]",            // 悬停时药丸跟随的目标
 *     activeSelector: `[data-nav="${active}"]` // 鼠标离开后药丸停靠的位置
 *   });
 *   return (
 *     <div ref={containerRef} {...containerProps} className="relative ...">
 *       ...按钮（须带 data-* 属性与 z-[1]）...
 *       <SlidingPill style={indicatorStyle} />
 *     </div>
 *   );
 *
 * 注意：容器内按钮必须 `relative z-[1]`（在药丸 z-0 之上），否则药丸会遮住文字。
 *
 * 测量用布局坐标（offsetTop/offsetHeight）而非 getBoundingClientRect：
 * morph 形变中容器被 framer-motion 以 transform: scale 变换，rect 返回被
 * 缩放的视口坐标（实测行高被压缩数倍），offset* 不受 transform 影响，
 * 任何时刻都返回真实布局值，因此药丸在面板打开瞬间即可正确显示。
 */
export function useSlidingIndicator({
  hoverSelector,
  activeSelector,
  followFocus = false,
}: {
  hoverSelector: string;
  activeSelector: string;
  followFocus?: boolean;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const hoveredRef = useRef<Element | null>(null);
  const focusedRef = useRef<Element | null>(null);
  const [pill, setPill] = useState<{ top: number; height: number } | null>(null);
  /* 首帧禁用过渡：面板打开时滑块直接以最终形态出现（跳过 height 0→67 的过渡），
     下一帧后恢复 class 过渡，hover 跟随/切换行仍平滑 */
  const [settled, setSettled] = useState(false);
  useEffect(() => {
    const t = window.setTimeout(() => setSettled(true), 0);
    return () => clearTimeout(t);
  }, []);

  const positionPillOn = useCallback((el: Element | null | undefined) => {
    if (!(el instanceof HTMLElement)) return;
    const container = containerRef.current;
    if (!container) return;
    setPill({
      top: el.offsetTop + container.scrollTop,
      height: el.offsetHeight,
    });
  }, []);

  const positionOnActive = useCallback(() => {
    const c = containerRef.current;
    if (!c) return;
    const el = c.querySelector(activeSelector);
    if (!el) {
      setPill(null);
      return;
    }
    positionPillOn(el);
  }, [activeSelector, positionPillOn]);

  const repositionPill = useCallback(() => {
    const target = hoveredRef.current ?? focusedRef.current;
    if (target) {
      positionPillOn(target);
    } else {
      positionOnActive();
    }
  }, [positionOnActive, positionPillOn]);

  /* 挂载 & 激活项变化时，药丸滑到激活项。
     useLayoutEffect：面板打开的首帧（paint 前）就同步测好位置，
     滑块直接以最终形态出现，不会先渲染空状态再过渡 */
  useLayoutEffect(() => {
    if (followFocus) repositionPill();
    else positionOnActive();
  }, [followFocus, positionOnActive, repositionPill]);

  /* 容器尺寸变化时重新测量（hover 时跟 hover 行） */
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const ro = new ResizeObserver(() => repositionPill());
    ro.observe(container);
    return () => ro.disconnect();
  }, [repositionPill]);

  const handleMouseOver = useCallback(
    (e: MouseEvent<HTMLDivElement>) => {
      const btn = (e.target as HTMLElement).closest(hoverSelector);
      if (btn) {
        hoveredRef.current = btn;
        positionPillOn(btn);
      }
    },
    [hoverSelector, positionPillOn],
  );

  const handleMouseLeave = useCallback(() => {
    hoveredRef.current = null;
    repositionPill();
  }, [repositionPill]);

  const handleFocus = useCallback((event: FocusEvent<HTMLDivElement>) => {
    const target = event.target.closest(hoverSelector);
    focusedRef.current = target?.matches(":focus-visible") ? target : null;
    if (focusedRef.current) hoveredRef.current = null;
    repositionPill();
  }, [hoverSelector, repositionPill]);

  const handleBlur = useCallback((event: FocusEvent<HTMLDivElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) {
      focusedRef.current = null;
      repositionPill();
    }
  }, [repositionPill]);

  return {
    containerRef,
    containerProps: {
      onMouseOver: handleMouseOver, onMouseLeave: handleMouseLeave,
      onFocusCapture: followFocus ? handleFocus : undefined,
      onBlurCapture: followFocus ? handleBlur : undefined,
    },
    indicatorStyle: {
      top: pill?.top ?? 0,
      height: pill?.height ?? 0,
      opacity: pill ? 1 : 0,
      /* 首帧无过渡（直接显示最终形态），settled 后走 class 的 transition-all */
      transition: settled ? undefined : "none",
    } as CSSProperties,
  };
}

/** 静默着色药丸 —— 默认 ui-tint；侧栏传 className="bg-sidebar-highlight" */
const PILL_BASE = `ui-hover-highlight absolute left-3 right-3 z-0 rounded-md pointer-events-none ${MOTION.standard}`;

type PillMotion = { duration: number; ease: string; exitDuration: number; enterDuration: number };
type PillTarget = { top: number; height: number; visible: boolean; immediate: boolean };

function createPillMotion(node: HTMLDivElement, motion: PillMotion) {
  const media = window.matchMedia("(prefers-reduced-motion: reduce)");
  const context = gsap.context(() => {}, node);
  let target: PillTarget = { top: 0, height: 0, visible: false, immediate: true };
  const paint = () => context.add(() => {
    gsap.killTweensOf(node);
    if (media.matches || target.immediate) {
      gsap.set(node, { y: target.top, height: target.height, opacity: target.visible ? 1 : 0 });
    } else if (!target.visible) {
      gsap.to(node, { opacity: 0, duration: motion.exitDuration, overwrite: true });
    } else if (Number(gsap.getProperty(node, "opacity")) <= .01) {
      gsap.set(node, { y: target.top, height: target.height, opacity: 0 });
      gsap.to(node, { opacity: 1, duration: motion.enterDuration, overwrite: true });
    } else {
      gsap.to(node, { y: target.top, height: target.height, opacity: 1,
        duration: motion.duration, ease: motion.ease, overwrite: true });
    }
  });
  const reduce = () => {
    gsap.killTweensOf(node);
    context.add(() => gsap.set(node, { y: target.top, height: target.height, opacity: target.visible ? 1 : 0 }));
  };
  media.addEventListener("change", reduce);
  return {
    moveTo(next: PillTarget) { target = next; paint(); },
    dispose() { media.removeEventListener("change", reduce); context.revert(); },
  };
}

export function SlidingPill({ style, className, motion }: { style: CSSProperties; className?: string; motion?: PillMotion }) {
  const nodeRef = useRef<HTMLDivElement>(null);
  const controller = useRef<ReturnType<typeof createPillMotion> | null>(null);
  useLayoutEffect(() => {
    if (!motion || !nodeRef.current) return;
    const instance = createPillMotion(nodeRef.current, motion);
    controller.current = instance;
    return () => { instance.dispose(); controller.current = null; };
  }, [motion]);
  useLayoutEffect(() => {
    controller.current?.moveTo({ top: Number(style.top ?? 0), height: Number(style.height ?? 0),
      visible: Number(style.opacity ?? 0) > 0, immediate: style.transition === "none" });
  }, [motion, style.top, style.height, style.opacity, style.transition]);
  const hasBgClass = Boolean(className && /\bbg-/.test(className));
  const { top: _top, height: _height, opacity: _opacity, ...rest } = style;
  return (
    <div
      ref={nodeRef}
      aria-hidden
      className={cn(PILL_BASE, !hasBgClass && "bg-ui-tint", className, motion && "transition-none")}
      style={motion ? { ...rest, top: 0, transition: "none", willChange: "transform" } : style}
    />
  );
}
