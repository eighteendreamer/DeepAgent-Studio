import React, { useState, useRef, useEffect, ReactNode } from 'react';

export interface TabItem {
  id: string;
  label: ReactNode;
}

export interface SlidingTabsProps {
  /**
   * The list of tabs to render.
   */
  tabs: TabItem[];
  
  /**
   * The ID of the currently active tab.
   */
  activeId: string;
  
  /**
   * Callback fired when a tab is clicked.
   */
  onChange: (id: string) => void;
  
  /**
   * Additional class names for the outer container.
   */
  className?: string;
  
  /**
   * Additional class names for individual tab buttons.
   */
  tabClassName?: string;
}

/**
 * 经典微浮雕滑动导航 (Classic Pill Shared Layout)
 * 采用第一性原理，利用绝对定位和 transform: translate 实现平滑过渡，性能极佳。
 */
export function SlidingTabs({
  tabs,
  activeId,
  onChange,
  className = '',
  tabClassName = '',
}: SlidingTabsProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const [indicatorStyle, setIndicatorStyle] = useState<React.CSSProperties>({ opacity: 0 });

  // 如果鼠标在悬停，滑块就追踪悬停目标；否则回到当前激活状态
  const targetId = hoveredId || activeId;

  useEffect(() => {
    if (!containerRef.current || !targetId) {
      setIndicatorStyle({ opacity: 0 });
      return;
    }

    const container = containerRef.current;
    // 找到目标元素的 DOM 节点
    const targetEl = container.querySelector(`[data-id="${targetId}"]`) as HTMLElement;

    if (targetEl) {
      setIndicatorStyle({
        opacity: 1,
        width: `${targetEl.offsetWidth}px`,
        // left + targetEl 的 offsetLeft。容器内部应该是相对定位
        transform: `translateX(${targetEl.offsetLeft}px)`,
        // 经典的 cubic-bezier，带有轻微阻尼感
        transition:
          'transform 0.35s cubic-bezier(0.4, 0, 0.2, 1), width 0.35s cubic-bezier(0.4, 0, 0.2, 1), opacity 0.2s ease',
      });
    } else {
      setIndicatorStyle({ opacity: 0 });
    }
  }, [targetId, tabs]); // 当 targetId 变化时重新计算坐标

  return (
    <div
      ref={containerRef}
      // isolate 隔离层级，防止动画溢出影响其他堆叠上下文
      className={`relative inline-flex items-center rounded-xl bg-hover-bg p-1.5 isolate ${className}`}
      onMouseLeave={() => setHoveredId(null)}
    >
      {/* ======================= */}
      {/* 滑块指示器 (Indicator)    */}
      {/* ======================= */}
      <div
        className="absolute top-1.5 bottom-1.5 left-0 z-0 rounded-lg bg-elevated-bg shadow-[0_2px_8px_rgba(0,0,0,0.06)] pointer-events-none dark:border dark:border-border-theme/50"
        style={indicatorStyle}
      />

      {/* ======================= */}
      {/* 选项卡按钮 (Tabs)        */}
      {/* ======================= */}
      {tabs.map((tab) => {
        const isActive = activeId === tab.id;
        const isHovered = hoveredId === tab.id;

        return (
          <button
            key={tab.id}
            data-id={tab.id}
            onClick={() => onChange(tab.id)}
            onMouseEnter={() => setHoveredId(tab.id)}
            // 按钮背景必须透明，依赖底层的滑块来提供背景
            className={`
              relative z-10 px-4 py-1.5 text-[0.95rem] font-medium transition-colors duration-200 rounded-lg outline-none
              ${isActive || isHovered ? 'text-text-base' : 'text-text-secondary'}
              ${tabClassName}
            `}
          >
            {tab.label}
          </button>
        );
      })}
    </div>
  );
}
