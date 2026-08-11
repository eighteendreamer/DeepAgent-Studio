import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type MouseEvent } from "react";
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
}: {
  hoverSelector: string;
  activeSelector: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const hoveredRef = useRef<Element | null>(null);
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
    if (hoveredRef.current) {
      positionPillOn(hoveredRef.current);
    } else {
      positionOnActive();
    }
  }, [positionOnActive, positionPillOn]);

  /* 挂载 & 激活项变化时，药丸滑到激活项。
     useLayoutEffect：面板打开的首帧（paint 前）就同步测好位置，
     滑块直接以最终形态出现，不会先渲染空状态再过渡 */
  useLayoutEffect(() => {
    positionOnActive();
  }, [positionOnActive]);

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
    positionOnActive();
  }, [positionOnActive]);

  return {
    containerRef,
    containerProps: { onMouseOver: handleMouseOver, onMouseLeave: handleMouseLeave },
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
const PILL_BASE = `absolute left-3 right-3 z-0 rounded-md pointer-events-none ${MOTION.standard}`;

export function SlidingPill({ style, className }: { style: CSSProperties; className?: string }) {
  const hasBgClass = Boolean(className && /\bbg-/.test(className));
  return (
    <div
      aria-hidden
      className={cn(PILL_BASE, !hasBgClass && "bg-ui-tint", className)}
      style={style}
    />
  );
}
