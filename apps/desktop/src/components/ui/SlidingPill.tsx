import { useCallback, useEffect, useRef, useState, type CSSProperties, type MouseEvent } from "react";
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
 */
export function useSlidingIndicator({ hoverSelector, activeSelector }: { hoverSelector: string; activeSelector: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [pill, setPill] = useState<{ top: number; height: number } | null>(null);

  const positionPillOn = useCallback((el: Element | null | undefined) => {
    if (!(el instanceof HTMLElement)) return;
    const container = containerRef.current;
    if (!container) return;
    const containerRect = container.getBoundingClientRect();
    const elRect = el.getBoundingClientRect();
    setPill({
      top: elRect.top - containerRect.top + container.scrollTop,
      height: elRect.height,
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

  /* 挂载 & 激活项变化时，药丸滑到激活项 */
  useEffect(() => {
    positionOnActive();
  }, [positionOnActive]);

  const handleMouseOver = useCallback(
    (e: MouseEvent<HTMLDivElement>) => {
      const btn = (e.target as HTMLElement).closest(hoverSelector);
      if (btn) positionPillOn(btn);
    },
    [hoverSelector, positionPillOn],
  );

  const handleMouseLeave = useCallback(() => {
    positionOnActive();
  }, [positionOnActive]);

  return {
    containerRef,
    containerProps: { onMouseOver: handleMouseOver, onMouseLeave: handleMouseLeave },
    indicatorStyle: {
      top: pill?.top ?? 0,
      height: pill?.height ?? 0,
      opacity: pill ? 1 : 0,
    } as CSSProperties,
  };
}

/** 静默着色药丸本体（无边框、无阴影，bg-black/5 与 hover-bg 语言一致） */
const PILL_CLASS = `absolute left-3 right-3 z-0 rounded-md bg-black/5 pointer-events-none ${MOTION.standard}`;

export function SlidingPill({ style, className }: { style: CSSProperties; className?: string }) {
  return <div aria-hidden className={cn(PILL_CLASS, className)} style={style} />;
}
