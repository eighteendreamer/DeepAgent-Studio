import type { HTMLAttributes } from "react";
import { cn } from "../shadcn/utils";
import { MOTION } from "./motion";

/**
 * 输入区容器（Composer 等）—— 统一语言：无边框、bg-elevated-bg + 柔和阴影，
 * 聚焦时阴影增强作为反馈（不靠边框）。
 */
export function InputSurface({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "relative w-full rounded-[24px] bg-elevated-bg shadow-[0_6px_20px_rgba(31,38,48,0.08)]",
        MOTION.standard,
        "focus-within:shadow-[0_8px_26px_rgba(31,38,48,0.12)]",
        className,
      )}
      {...rest}
    />
  );
}
