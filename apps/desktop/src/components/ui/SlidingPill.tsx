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
export function useSlidingIndicator({
  hoverSelector,
  activeSelector,
  layoutAnimating = false,
  hoverOnly = false,
}: {
  hoverSelector: string;
  activeSelector: string;
  /** Morph 面板展开中：每帧重测，药丸跟随行位（不隐藏） */
  layoutAnimating?: boolean;
  /** 无固定激活项（activeId=__none__）：形变期间不显示药丸，避免 scale 测量错位 */
  hoverOnly?: boolean;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const hoveredRef = useRef<Element | null>(null);
  const [hovering, setHovering] = useState(false);
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

  const repositionPill = useCallback(() => {
    if (hoveredRef.current) {
      positionPillOn(hoveredRef.current);
    } else {
      positionOnActive();
    }
  }, [positionOnActive, positionPillOn]);

  /* 纯 hover 菜单：无 hover 时形变开始清药丸 */
  useEffect(() => {
    if (layoutAnimating && hoverOnly && !hovering) {
      hoveredRef.current = null;
      setPill(null);
    }
  }, [layoutAnimating, hoverOnly, hovering]);

  /* 纯 hover + 形变中 + 有 hover：每帧跟鼠标行（scale 期间测量准确） */
  useEffect(() => {
    if (!layoutAnimating || !hoverOnly || !hovering) return;
    let rafId = 0;
    const start = performance.now();
    const tick = () => {
      if (hoveredRef.current) {
        positionPillOn(hoveredRef.current);
      }
      if (performance.now() - start < 700) {
        rafId = requestAnimationFrame(tick);
      }
    };
    rafId = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafId);
  }, [layoutAnimating, hoverOnly, hovering, positionPillOn]);

  /* 挂载 & 激活项变化时，药丸滑到激活项 */
  useEffect(() => {
    positionOnActive();
  }, [positionOnActive]);

  /* morph / 面板尺寸变化时重新测量（hover 时跟 hover 行） */
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const ro = new ResizeObserver(() => repositionPill());
    ro.observe(container);
    return () => ro.disconnect();
  }, [repositionPill]);

  /* 形变结束：等 layout 稳定后再重测（双 rAF） */
  useEffect(() => {
    if (layoutAnimating) return;
    let outer = 0;
    let inner = 0;
    outer = requestAnimationFrame(() => {
      inner = requestAnimationFrame(() => repositionPill());
    });
    return () => {
      cancelAnimationFrame(outer);
      cancelAnimationFrame(inner);
    };
  }, [layoutAnimating, repositionPill]);

  /* 形变期间且无 hover：每帧跟激活项；有 hover 时交给 CSS transition + ResizeObserver */
  useEffect(() => {
    if (!layoutAnimating) return;
    let rafId = 0;
    const start = performance.now();
    const tick = () => {
      if (!hoveredRef.current) {
        positionOnActive();
      }
      if (performance.now() - start < 700) {
        rafId = requestAnimationFrame(tick);
      }
    };
    rafId = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafId);
  }, [layoutAnimating, positionOnActive]);

  const handleMouseOver = useCallback(
    (e: MouseEvent<HTMLDivElement>) => {
      const btn = (e.target as HTMLElement).closest(hoverSelector);
      if (btn) {
        hoveredRef.current = btn;
        setHovering(true);
        positionPillOn(btn);
      }
    },
    [hoverSelector, positionPillOn],
  );

  const handleMouseLeave = useCallback(() => {
    hoveredRef.current = null;
    setHovering(false);
    positionOnActive();
  }, [positionOnActive]);

  return {
    containerRef,
    containerProps: { onMouseOver: handleMouseOver, onMouseLeave: handleMouseLeave },
    indicatorStyle: {
      top: pill?.top ?? 0,
      height: pill?.height ?? 0,
      opacity: pill && !(hoverOnly && layoutAnimating && !hovering) ? 1 : 0,
      /* 形变期间跟 hover/激活项时用 rAF，关闭 CSS 过渡避免滞后 */
      transition: layoutAnimating ? "none" : undefined,
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
