import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { motion, useReducedMotion } from "framer-motion";
import { cn } from "../shadcn/utils";
import {
  morphListVariants,
  morphSpringTransition,
} from "./morphingMenuMotion";
import { MorphPanelLayoutContext } from "./MorphPanelLayoutContext";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  layoutId: string;
  trigger: ReactNode;
  className?: string;
  panelClassName?: string;
  /** 面板在触发器上方左/右对齐 */
  panelAlign?: "left" | "right";
  zIndex?: number;
  staggerContent?: boolean;
  /** 不包 elevated 外壳（子内容自带 Panel） */
  unstyled?: boolean;
  children: ReactNode;
};

const PANEL_ANCHOR = {
  left: "absolute bottom-full left-0 mb-2 origin-bottom-left",
  right: "absolute bottom-full right-0 mb-2 origin-bottom-right",
} as const;

/**
 * 通用触发器 ↔ 浮层 morph（layoutId 形变）。
 * Composer 胶囊 / Footer 工具栏均可用。
 */
export function MorphingMenuShell({
  open,
  onOpenChange,
  layoutId,
  trigger,
  className,
  panelClassName,
  panelAlign = "left",
  zIndex = 50,
  staggerContent = false,
  unstyled = false,
  children,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const reduced = useReducedMotion();
  const spring = morphSpringTransition(reduced);
  const listVariants = morphListVariants(reduced);
  const [layoutAnimating, setLayoutAnimating] = useState(false);

  useLayoutEffect(() => {
    if (!open || reduced) {
      setLayoutAnimating(false);
      return;
    }
    setLayoutAnimating(true);
  }, [open, reduced]);

  useEffect(() => {
    if (!open || reduced) return;
    const fallback = window.setTimeout(() => setLayoutAnimating(false), 680);
    return () => clearTimeout(fallback);
  }, [open, reduced]);

  useEffect(() => {
    if (!open) return;
    const onOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        onOpenChange(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onOpenChange(false);
    };
    document.addEventListener("mousedown", onOutside);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onOutside);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onOpenChange]);

  const shellClass = unstyled
    ? cn(panelClassName, layoutAnimating && "!overflow-hidden")
    : cn(
        "overflow-hidden rounded-2xl bg-elevated-bg shadow-[0_6px_24px_rgba(0,0,0,0.10)]",
        panelClassName,
        layoutAnimating && "!overflow-hidden",
      );

  return (
    <div ref={containerRef} className={cn("relative", className)}>
      {open && (
        <div className="invisible inline-flex max-w-full pointer-events-none" aria-hidden>
          {trigger}
        </div>
      )}

      <motion.div
        layoutId={layoutId}
        transition={spring}
        style={{ zIndex: open ? zIndex : undefined }}
        onLayoutAnimationComplete={() => {
          if (open) setLayoutAnimating(false);
        }}
        className={cn(
          open
            ? cn(shellClass, PANEL_ANCHOR[panelAlign])
            : "inline-flex max-w-full origin-bottom-left",
        )}
      >
        {!open ? (
          trigger
        ) : (
          <MorphPanelLayoutContext.Provider value={layoutAnimating}>
            {staggerContent ? (
              <motion.div initial="hidden" animate="visible" variants={listVariants}>
                {children}
              </motion.div>
            ) : (
              children
            )}
          </MorphPanelLayoutContext.Provider>
        )}
      </motion.div>
    </div>
  );
}
