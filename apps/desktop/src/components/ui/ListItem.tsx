import type { HTMLAttributes } from "react";
import { cn } from "../shadcn/utils";
import { FLOATING_MENU, MOTION } from "./motion";

/**
 * 列表项 / 下拉行 —— 统一语言：hover 与激活同为 bg-ui-tint（主题感知静默着色）。
 * selected 时加粗（与侧栏激活项一致）；不需要浮层/边框。
 */
export function ListItem({
  selected = false,
  sliding = false,
  className,
  ...rest
}: HTMLAttributes<HTMLDivElement> & { selected?: boolean; sliding?: boolean }) {
  return (
    <div
      className={cn(
        "flex w-full items-center justify-between rounded-lg px-2 py-1.5 text-[12px]",
        !sliding && FLOATING_MENU.item,
        MOTION.fast,
        sliding
          ? "relative z-[1]"
          : selected
            ? "bg-ui-tint font-medium"
            : "hover:bg-ui-tint",
        sliding && selected && "font-medium text-text-base",
        className,
      )}
      {...rest}
    />
  );
}
