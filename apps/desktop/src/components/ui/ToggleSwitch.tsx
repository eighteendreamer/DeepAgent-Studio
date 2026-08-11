import type { ButtonHTMLAttributes } from "react";
import { cn } from "../shadcn/utils";
import { MOTION } from "./motion";

type ToggleSwitchSize = "sm" | "md";
type ToggleSwitchTone = "primary" | "success";

const SIZE: Record<
  ToggleSwitchSize,
  { track: string; thumb: string; thumbOn: string; thumbOff: string }
> = {
  sm: {
    track: "h-5 w-9",
    thumb: "h-4 w-4 top-[2px]",
    thumbOff: "translate-x-[2px]",
    thumbOn: "translate-x-[18px]",
  },
  md: {
    track: "h-7 w-12",
    thumb: "h-6 w-6 top-[2px]",
    thumbOff: "translate-x-[2px]",
    thumbOn: "translate-x-[22px]",
  },
};

export function ToggleSwitch({
  checked,
  onChange,
  size = "sm",
  tone = "primary",
  className,
  disabled,
  ...rest
}: {
  checked: boolean;
  onChange: () => void;
  size?: ToggleSwitchSize;
  tone?: ToggleSwitchTone;
} & Omit<ButtonHTMLAttributes<HTMLButtonElement>, "onChange">) {
  const s = SIZE[size];

  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={(event) => {
        event.stopPropagation();
        onChange();
      }}
      className={cn(
        "relative shrink-0 rounded-full border-0 p-0 outline-none",
        s.track,
        MOTION.standard,
        "focus-visible:ring-2 focus-visible:ring-primary/30",
        checked
          ? tone === "success"
            ? "bg-green-500"
            : "bg-text-base"
          : "bg-ui-tint-strong",
        disabled && "cursor-not-allowed opacity-45",
        className,
      )}
      {...rest}
    >
      <span
        aria-hidden
        className={cn(
          "pointer-events-none absolute left-0 rounded-full bg-white shadow-sm",
          s.thumb,
          MOTION.standard,
          checked ? s.thumbOn : s.thumbOff,
        )}
      />
    </button>
  );
}

export function ToggleSwitchRow({
  checked,
  onChange,
  label,
  hint,
  size = "md",
  tone = "success",
}: {
  checked: boolean;
  onChange: () => void;
  label: string;
  hint?: string;
  size?: ToggleSwitchSize;
  tone?: ToggleSwitchTone;
}) {
  return (
    <label
      onClick={(e) => {
        e.preventDefault();
        onChange();
      }}
      className={cn(
        "flex cursor-pointer select-none items-center",
        size === "sm" ? "gap-2" : "gap-3",
        MOTION.fast,
      )}
      title={hint}
    >
      <ToggleSwitch checked={checked} onChange={onChange} size={size} tone={tone} />
      <span
        className={cn(
          "shrink-0 whitespace-nowrap text-[13px] leading-snug",
          checked ? "text-text-base" : "text-text-secondary",
        )}
      >
        {label}
      </span>
    </label>
  );
}
