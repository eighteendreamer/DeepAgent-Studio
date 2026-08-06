import type { HTMLAttributes } from "react";
import { cn } from "../shadcn/utils";
import { FLOATING_MENU } from "./motion";

/**
 * 浮层容器（下拉菜单/弹层面板）—— 统一语言：无边框、bg-elevated-bg + 大扩散阴影。
 * menu=true 时附加入场动效（方案 H）；模态框等传 menu={false}。
 */
export function Panel({
  className,
  menu = true,
  ...rest
}: HTMLAttributes<HTMLDivElement> & { menu?: boolean }) {
  return (
    <div
      className={cn(
        "rounded-2xl bg-elevated-bg shadow-[0_6px_24px_rgba(0,0,0,0.10)]",
        menu && FLOATING_MENU.panel,
        className,
      )}
      {...rest}
    />
  );
}
