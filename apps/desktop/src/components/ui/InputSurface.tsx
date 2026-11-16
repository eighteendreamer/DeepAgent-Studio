import type { HTMLAttributes } from "react";
import { cn } from "../shadcn/utils";

/**
 * 输入区容器（Composer 等）—— 统一语言：无边框、bg-elevated-bg + 柔和阴影，
 * 聚焦时阴影增强作为反馈（不靠边框）。
 *
 * The transition deliberately excludes `height`: the composer grows to fit its
 * draft, and animating that growth turned a single stale measurement into a
 * slow, visible climb. Colours and shadow still ease; geometry changes are
 * applied immediately so the box is never mid-transition while being measured.
 */
export function InputSurface({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "relative w-full rounded-[24px] bg-elevated-bg shadow-[0_6px_20px_rgba(31,38,48,0.08)]",
        "transition-[box-shadow,background-color,border-color] duration-300 ease-out",
        "focus-within:shadow-[0_8px_26px_rgba(31,38,48,0.12)]",
        className,
      )}
      {...rest}
    />
  );
}
