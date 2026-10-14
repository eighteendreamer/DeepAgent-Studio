import { useLayoutEffect, useRef, type ReactNode } from "react";
import { ChevronRight } from "lucide-react";
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
  const contentRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const content = contentRef.current;
    if (!content) return;
    if (!open && content.contains(document.activeElement)) {
      content.closest("[data-process-turn]")?.querySelector<HTMLButtonElement>("[data-process-toggle]")?.focus({ preventScroll: true });
    }
    content.inert = !open;
  }, [open]);
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
          ref={contentRef}
          aria-hidden={!open}
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
    <ChevronRight
      size={12}
      className={cn(
        "shrink-0 opacity-40 transition-transform duration-300 ease-out group-hover:opacity-70",
        open && "rotate-90",
        className,
      )}
    />
  );
}
