import type { ButtonHTMLAttributes, ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { cn } from "../shadcn/utils";
import { MOTION } from "./motion";

/**
 * Composer 底栏工具触发器 —— 项目 / 环境 / Git 同级入口。
 * 固定 h-8、图标列 w-4、文案 truncate，保证三项对齐一致。
 */
export function ToolbarMenuTrigger({
  open = false,
  icon,
  label,
  trailing,
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  open?: boolean;
  icon: IconProp;
  label: ReactNode;
  trailing?: ReactNode;
}) {
  return (
    <button
      type="button"
      className={cn(
        "inline-flex h-8 max-w-[200px] shrink-0 items-center gap-1.5 text-[12px] font-medium text-text-secondary",
        MOTION.fast,
        "hover:text-text-base",
        open && "text-text-base",
        className,
      )}
      {...rest}
    >
      <span className="flex h-4 w-4 shrink-0 items-center justify-center text-text-secondary">
        <FontAwesomeIcon icon={icon} className="text-[13px]" />
      </span>
      <span className="min-w-0 truncate">{label}</span>
      {trailing}
      <FontAwesomeIcon
        icon={["fas", "chevron-down"]}
        className="shrink-0 text-[9px] text-text-secondary opacity-70"
      />
    </button>
  );
}
