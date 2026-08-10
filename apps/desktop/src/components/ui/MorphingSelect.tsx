import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { cn } from "../shadcn/utils";
import {
  morphItemVariants,
  morphListVariants,
  morphSpringTransition,
} from "./morphingMenuMotion";

export type MorphingSelectOption = {
  id: string;
  label: ReactNode;
};

type Props = {
  value: string | null;
  onChange: (id: string) => void;
  options: MorphingSelectOption[];
  disabled?: boolean;
  placeholder?: string;
  className?: string;
  panelClassName?: string;
  panelWidth?: number;
  /** 交错延迟（秒），默认 0.02 */
  stagger?: number;
  /** spring 时长（秒），默认 0.62 */
  duration?: number;
};

function findOption(options: MorphingSelectOption[], id: string | null) {
  if (!id) return null;
  return options.find((o) => o.id === id) ?? null;
}

export function MorphingSelect({
  value,
  onChange,
  options,
  disabled = false,
  placeholder = "Select…",
  className,
  panelClassName,
  panelWidth = 280,
  stagger = 0.02,
  duration = 0.62,
}: Props) {
  const uid = useId().replace(/:/g, "");
  const layoutScope = `morph-select-${uid}`;
  const containerRef = useRef<HTMLDivElement>(null);

  const reduced = useReducedMotion();
  const selected = findOption(options, value);

  const [isOpen, setIsOpen] = useState(false);

  const springTransition = morphSpringTransition(reduced, duration);
  const listVariants = morphListVariants(reduced, stagger);
  const itemVariants = morphItemVariants(reduced);

  useEffect(() => {
    if (!isOpen) return;
    const onOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setIsOpen(false);
    };
    document.addEventListener("mousedown", onOutside);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onOutside);
      document.removeEventListener("keydown", onKey);
    };
  }, [isOpen]);

  const shellClass = cn(
    "bg-elevated-bg shadow-[0_6px_24px_rgba(0,0,0,0.10)]",
    panelClassName,
  );

  const handleSelect = (id: string) => {
    if (id !== value) onChange(id);
    setIsOpen(false);
  };

  return (
    <div
      ref={containerRef}
      className={cn("relative inline-flex", className)}
    >
      <AnimatePresence mode="wait" initial={false}>
        {!isOpen ? (
          <motion.button
            key="pill"
            layoutId={layoutScope}
            type="button"
            className={cn(
              shellClass,
              "flex h-9 max-w-full items-center justify-between gap-2 rounded-full px-3.5 text-left text-[13px] font-medium text-text-base",
              disabled ? "cursor-not-allowed opacity-50" : "cursor-pointer",
            )}
            onClick={() => !disabled && setIsOpen(true)}
            whileHover={!disabled && !reduced ? { scale: 1.02 } : undefined}
            whileTap={!disabled && !reduced ? { scale: 0.96 } : undefined}
            transition={springTransition}
            aria-haspopup="listbox"
            aria-expanded={isOpen}
            disabled={disabled}
          >
            <motion.span
              layout="position"
              className="min-w-0 truncate"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: reduced ? 0 : 0.2 }}
            >
              {selected ? selected.label : placeholder}
            </motion.span>
            <FontAwesomeIcon
              icon={["fas", "chevron-down"]}
              className="shrink-0 text-[9px] text-text-secondary opacity-70"
            />
          </motion.button>
        ) : (
          <motion.div
            key="card"
            layoutId={layoutScope}
            className={cn(shellClass, "w-full overflow-hidden rounded-2xl")}
            style={{ width: panelWidth }}
            transition={springTransition}
            role="listbox"
          >
            <motion.ul
              className="list-none p-1.5"
              initial="hidden"
              animate="visible"
              exit="hidden"
              variants={listVariants}
            >
              {options.map((option) => {
                const isSelected = option.id === value;
                return (
                  <motion.li
                    key={option.id}
                    role="option"
                    aria-selected={isSelected}
                    tabIndex={0}
                    className={cn(
                      "relative flex h-10 cursor-pointer items-center justify-between rounded-xl px-3 text-[13px] text-text-base",
                      isSelected ? "bg-ui-tint font-medium" : "hover:bg-ui-tint",
                    )}
                    onClick={() => handleSelect(option.id)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        handleSelect(option.id);
                      }
                    }}
                    variants={itemVariants}
                  >
                    <span className="min-w-0 truncate">{option.label}</span>
                    {isSelected && (
                      <motion.span
                        layoutId={`${layoutScope}-indicator`}
                        className="ml-2 h-1.5 w-1.5 shrink-0 rounded-full bg-text-base"
                        aria-hidden
                      />
                    )}
                  </motion.li>
                );
              })}
            </motion.ul>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
