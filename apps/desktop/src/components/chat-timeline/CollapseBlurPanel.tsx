import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { cn } from "../shadcn/utils";

/** K 方案：grid 高度 + blur/opacity 聚焦揭示 */
export function CollapseBlurPanel({
  open,
  className,
  innerClassName,
  children,
}: {
  open: boolean;
  className?: string;
  innerClassName?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "grid transition-[grid-template-rows] duration-300 ease-out motion-reduce:transition-none",
        open ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
        className,
      )}
    >
      <div className="min-h-0 overflow-hidden">
        <div
          className={cn(
            "transition-[filter,opacity] duration-[420ms] ease-out motion-reduce:blur-none motion-reduce:opacity-100 motion-reduce:transition-none",
            open ? "blur-0 opacity-100" : "pointer-events-none blur-[8px] opacity-0",
            innerClassName,
          )}
        >
          {children}
        </div>
      </div>
    </div>
  );
}

export function CollapseChevron({ open, className }: { open: boolean; className?: string }) {
  return (
    <FontAwesomeIcon
      icon={["fas", "chevron-right"]}
      className={cn(
        "shrink-0 text-[10px] opacity-40 transition-transform duration-300 ease-out group-hover:opacity-70",
        open && "rotate-90",
        className,
      )}
    />
  );
}
