import { useLayoutEffect, useRef, type ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "../shadcn/utils";
import { SpringDisclosure } from "../ui/SpringDisclosure";

/** Process details share the restrained layout spring. */
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
    <SpringDisclosure open={open} className={className}>
        <div
          ref={contentRef}
          aria-hidden={!open}
          className={cn(!open && "pointer-events-none", innerClassName)}
        >
          {children}
        </div>
    </SpringDisclosure>
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
