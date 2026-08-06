import type { ButtonHTMLAttributes } from "react";
import { cn } from "../shadcn/utils";
import { MOTION } from "./motion";

/**
 * 着色按钮 —— 统一语言：无边框、bg-black/5 着色底、hover 同色加深。
 * variant="primary" 为品牌色实心按钮（发送/主操作）。
 */
export function TintButton({
  variant = "default",
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "default" | "primary" }) {
  return (
    <button
      className={cn(
        "rounded-full px-2 py-1.5 text-[12px] text-text-base",
        MOTION.fast,
        variant === "primary"
          ? "bg-primary text-white hover:bg-primary-hover"
          : "bg-black/5 hover:bg-black/5 disabled:opacity-50 disabled:cursor-not-allowed",
        className,
      )}
      {...rest}
    />
  );
}
