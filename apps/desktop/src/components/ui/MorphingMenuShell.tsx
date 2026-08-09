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
  /** 面板在触发器上方/下方，左/右对齐 */
  panelAlign?: "left" | "right";
  /** above：Composer/Footer 向上展开；below：侧栏 ⋯ 向下展开 */
  panelPlacement?: "above" | "below";
  zIndex?: number;
  staggerContent?: boolean;
  /** 不包 elevated 外壳（子内容自带 Panel） */
  unstyled?: boolean;
  /** inline：Composer/Footer 胶囊；block：侧栏整行触发器 */
  triggerLayout?: "inline" | "block";
  children: ReactNode;
};

const TRIGGER_LAYOUT = {
  inline: "inline-flex max-w-full",
  block: "flex w-full",
} as const;

const PANEL_ANCHOR = {
  above: {
    left: "absolute bottom-full left-0 mb-2 origin-bottom-left",
    right: "absolute bottom-full right-0 mb-2 origin-bottom-right",
  },
  below: {
    left: "absolute top-full left-0 mt-1 origin-top-left",
    right: "absolute top-full right-0 mt-1 origin-top-right",
  },
} as const;

const CLOSED_ORIGIN = {
  above: {
    left: "origin-bottom-left",
    right: "origin-bottom-right",
  },
  below: {
    left: "origin-top-left",
    right: "origin-top-right",
  },
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
  panelPlacement = "above",
  zIndex = 50,
  staggerContent = false,
  unstyled = false,
  triggerLayout = "inline",
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
        "overflow-hidden rounded-2xl bg-elevated-bg",
        panelClassName,
        layoutAnimating && "!overflow-hidden",
      );

  const panelShadow =
    open && !reduced ? "drop-shadow(0 6px 24px rgba(0,0,0,0.10))" : undefined;

  return (
    <div ref={containerRef} className={cn("relative", className)}>
      {open && (
        <div
          className={cn("invisible pointer-events-none", TRIGGER_LAYOUT[triggerLayout])}
          aria-hidden
        >
          {trigger}
        </div>
      )}

      <motion.div
        layoutId={layoutId}
        transition={spring}
        style={{ zIndex: open ? zIndex : undefined, filter: panelShadow }}
        onLayoutAnimationComplete={() => {
          if (open) setLayoutAnimating(false);
        }}
        className={cn(
          open
            ? cn(shellClass, PANEL_ANCHOR[panelPlacement][panelAlign])
            : cn(TRIGGER_LAYOUT[triggerLayout], CLOSED_ORIGIN[panelPlacement][panelAlign]),
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
