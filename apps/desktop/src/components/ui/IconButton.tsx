import type { ButtonHTMLAttributes } from "react";
import { cn } from "../shadcn/utils";
import { MOTION } from "./motion";

/**
 * 图标按钮 —— 统一语言：无边框、透明底、hover 静默着色。
 * 用于工具栏、行内操作等小图标按钮。
 */
export function IconButton({ className, ...rest }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      className={cn(
        "flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-full text-text-secondary",
        MOTION.fast,
        "hover:bg-black/5 hover:text-text-base",
        "disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent",
        className,
      )}
      {...rest}
    />
  );
}
