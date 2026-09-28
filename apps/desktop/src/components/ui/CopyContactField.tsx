import { useEffect, useRef, useState } from "react";

import { Label } from "../shadcn/label";
import { cn } from "../shadcn/utils";
import { HoverInfo } from "./HoverInfo";
import { MOTION } from "./motion";

export interface CopyContactFieldProps {
  id: string;
  label: string;
  value: string;
  copyLabel: string;
  copiedLabel: string;
}

export function CopyContactField({ id, label, value, copyLabel, copiedLabel }: CopyContactFieldProps) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<number | null>(null);

  useEffect(() => () => {
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
  }, []);

  const handleCopy = async () => {
    if (!value || !navigator.clipboard?.writeText) return;
    try {
      await navigator.clipboard.writeText(value);
    } catch {
      return;
    }
    setCopied(true);
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => setCopied(false), 1200);
  };

  return (
    <div className="grid gap-2">
      <Label htmlFor={id}>{label}</Label>
      <HoverInfo content={copied ? copiedLabel : copyLabel}>
        <button
          type="button"
          id={id}
          className={cn(
            "contact-copy-field relative w-full rounded-md bg-ui-tint px-3.5 py-2.5 text-left cursor-copy",
            MOTION.fast,
            copied && "is-copied",
          )}
          onClick={() => void handleCopy()}
        >
          <span className="block text-[13px] leading-snug text-text-base break-all">{value}</span>
        </button>
      </HoverInfo>
    </div>
  );
}
