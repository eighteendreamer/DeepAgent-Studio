import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "../shadcn/utils";
import { MENU_LIST } from "./motion";
import { useSlidingIndicator, SlidingPill } from "./SlidingPill";
import { useMorphPanelLayoutAnimating } from "./MorphPanelLayoutContext";

/** 浮层菜单行统一 data 属性，配合 SlidingMenuList 使用 */
export const MENU_ITEM_ATTR = "data-menu-item";

type Props = HTMLAttributes<HTMLDivElement> & {
  activeId: string;
  pillClassName?: string;
  children: ReactNode;
};

/**
 * 浮层内列表滑动药丸 —— 与侧栏/设置侧栏同款交互：
 * 悬停跟随，离开滑回当前选中项；行元素须带 `data-menu-item={id}` 与 `relative z-[1]`。
 */
export function SlidingMenuList({
  activeId,
  pillClassName,
  className,
  children,
  ...rest
}: Props) {
  const layoutAnimating = useMorphPanelLayoutAnimating();
  const { containerRef, containerProps, indicatorStyle } = useSlidingIndicator({
    hoverSelector: `[${MENU_ITEM_ATTR}]`,
    activeSelector: `[${MENU_ITEM_ATTR}="${activeId}"]`,
    layoutAnimating,
  });

  return (
    <div ref={containerRef} {...containerProps} className={cn("relative w-full overflow-hidden", className)} {...rest}>
      {children}
      <SlidingPill
        className={cn("rounded-lg", MENU_LIST.pillInset, pillClassName)}
        style={indicatorStyle}
      />
    </div>
  );
}
