import { forwardRef, useCallback, useId, useLayoutEffect, useRef, type ReactNode } from "react";
import { motion, useIsPresent, useReducedMotion } from "framer-motion";
import type { LucideIcon } from "lucide-react";
import { Button } from "./shadcn/button";

export const ONBOARDING_EASE = [0.65, 0, 0.35, 1] as const;

// Animate measured height rather than scaling the card and its text.
export const OnboardingPanel = forwardRef<HTMLDivElement, {
  direction: number;
  disabled: boolean;
  onHeight: (height: number) => void;
  onEntered: () => void;
  children: ReactNode;
}>(function OnboardingPanel({ direction, disabled, onHeight, onEntered, children }, forwardedRef) {
  const ref = useRef<HTMLDivElement | null>(null);
  const present = useIsPresent();
  const reduced = useReducedMotion();
  const setRef = useCallback((node: HTMLDivElement | null) => {
    ref.current = node;
    if (typeof forwardedRef === "function") forwardedRef(node);
    else if (forwardedRef) forwardedRef.current = node;
  }, [forwardedRef]);

  useLayoutEffect(() => {
    const panel = ref.current;
    if (!panel) return;
    panel.toggleAttribute("inert", !present || disabled);
    if (!present) return;
    const measure = () => onHeight(panel.offsetHeight);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(panel);
    return () => observer.disconnect();
  }, [disabled, onHeight, present]);

  return (
    <motion.div
      ref={setRef}
      className="onboarding-panel w-full"
      custom={direction}
      initial="enter"
      animate="visible"
      exit="exit"
      variants={{
        enter: (way: number) => ({ opacity: 0, x: reduced ? 0 : 18 * way }),
        visible: { opacity: 1, x: 0, transition: { duration: reduced ? 0 : 0.24, delay: reduced ? 0 : 0.04 } },
        exit: (way: number) => ({ opacity: 0, x: reduced ? 0 : -18 * way, transition: { duration: reduced ? 0 : 0.18 } }),
      }}
      aria-hidden={!present || undefined}
      onAnimationComplete={(definition) => {
        if (present && definition === "visible") {
          ref.current?.removeAttribute("inert");
          onEntered();
          ref.current?.querySelector<HTMLHeadingElement>("h1")?.focus({ preventScroll: true });
        }
      }}
    >
      <fieldset disabled={disabled || !present} className="m-0 min-w-0 border-0 p-0">
        {children}
      </fieldset>
    </motion.div>
  );
});

type Choice<T extends string> = { id: T; title: string; description?: string; icon: LucideIcon; recommended?: boolean };

export function OnboardingChoices<T extends string>({ label, value, options, onChange, layout }: {
  label: string;
  value: T;
  options: Choice<T>[];
  onChange: (value: T) => void;
  layout: "theme" | "sandbox" | "work";
}) {
  const id = useId();
  const reduced = useReducedMotion();
  return (
    <div
      role="radiogroup"
      aria-label={label}
      className={`onboarding-choices onboarding-choices-${layout}`}
      onKeyDown={(event) => {
        if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        const index = options.findIndex(option => option.id === value);
        const delta = ["ArrowLeft", "ArrowUp"].includes(event.key) ? -1 : 1;
        const next = event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : (index + delta + options.length) % options.length;
        const button = event.currentTarget.querySelectorAll<HTMLButtonElement>("[role=radio]")[next];
        // A disabled fieldset also disables its descendants during transitions.
        if (button.matches(":disabled")) return;
        button.focus();
        onChange(options[next].id);
      }}
    >
      {options.map(option => (
        <Button
          key={option.id}
          variant="ghost"
          role="radio"
          aria-checked={value === option.id}
          tabIndex={value === option.id ? 0 : -1}
          onClick={() => onChange(option.id)}
          className="onboarding-choice"
        >
          {value === option.id && (
            <motion.span
              layoutId={`onboarding-selection-${id}`}
              className="onboarding-choice-surface"
              transition={{ duration: reduced ? 0 : 0.2, ease: ONBOARDING_EASE }}
              aria-hidden="true"
            />
          )}
          {layout !== "theme" && <span className="onboarding-radio" aria-hidden="true" />}
          <option.icon className="onboarding-choice-icon" aria-hidden="true" />
          <span className="onboarding-choice-copy">
            <span className="onboarding-choice-title">
              {option.title}
              {option.recommended && <span className="ml-2 text-[10px] font-normal text-text-secondary">推荐</span>}
            </span>
            {option.description && <span className="onboarding-choice-description">{option.description}</span>}
          </span>
        </Button>
      ))}
    </div>
  );
}
