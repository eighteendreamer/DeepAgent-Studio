import { HoverCard, HoverCardContent, HoverCardTrigger } from "./shadcn/hover-card";
import { useMemo, useState, useEffect, useRef } from "react";
import type { ContextUsageSnapshot } from "../types";
import { MOTION } from "./ui/motion";

interface Props {
  snapshot?: ContextUsageSnapshot | null;
  /**
   * Provider-resolved context window for the active model, from the backend
   * capability resolver. Used when there is no snapshot yet (e.g. an empty
   * session) so the denominator is the model's real capacity, never a
   * hardcoded guess. See deepagent-models::ModelCapabilityResolver.
   */
  contextWindow?: number;
  fallbackPromptTokens?: number;
  /** Hide popover while another toolbar overlay is open. */
  popoverSuppressed?: boolean;
  /** Increment to force-close the popover. */
  overlayCloseSignal?: number;
  onPopoverOpenChange?: (open: boolean) => void;
}

function formatTokens(value: number): string {
  if (value >= 1_000_000) {
    const m = value / 1_000_000;
    return `${Number.isInteger(m) ? m.toFixed(0) : m.toFixed(1)}M`;
  }
  if (value >= 1_000) {
    const k = value / 1_000;
    return `${Number.isInteger(k) ? k.toFixed(0) : k.toFixed(1)}k`;
  }
  return String(Math.max(0, Math.round(value)));
}

function capacityTone(ratio: number, isEmptySession: boolean): string {
  if (isEmptySession) return "#cbd5e1";
  if (ratio >= 0.92) return "#ef4444";
  if (ratio >= 0.8) return "#f59e0b";
  if (ratio >= 0.6) return "#3b82f6";
  return "#94a3b8";
}

export function ContextCapacityIndicator({
  snapshot,
  contextWindow: capabilityContextWindow = 0,
  fallbackPromptTokens = 0,
  popoverSuppressed = false,
  overlayCloseSignal = 0,
  onPopoverOpenChange,
}: Props) {
  const [open, setOpen] = useState(false);
  const lastOverlayCloseSignal = useRef(overlayCloseSignal);
  const lastReportedOpen = useRef(false);
  // The run's own snapshot wins (the runtime produced it from the same policy),
  // then the provider-resolved capability. Never a hardcoded cap.
  const contextWindow = snapshot?.context_window ?? capabilityContextWindow;
  const hasWindow = contextWindow > 0;
  const usedTokens = snapshot?.estimated_prompt_tokens ?? Math.max(0, Math.round(fallbackPromptTokens));
  const isEmptySession = !snapshot && usedTokens === 0;
  // Without a denominator there is no meaningful occupancy — show an empty ring
  // rather than dividing by a placeholder.
  const ratio = !hasWindow
    ? 0
    : Math.max(0, Math.min(1, snapshot?.used_ratio ?? usedTokens / contextWindow));
  const percent = Math.round(ratio * 100);
  const stroke = capacityTone(ratio, isEmptySession);
  const circumference = 2 * Math.PI * 8.5;
  const dashOffset = circumference * (1 - ratio);
  const blocks = useMemo(
    () => [...(snapshot?.blocks ?? [])].sort((a, b) => b.tokens - a.tokens).slice(0, 3),
    [snapshot?.blocks],
  );
  const cacheTotal = (snapshot?.cache_hit_tokens ?? 0) + (snapshot?.cache_miss_tokens ?? 0);
  const cacheRatio =
    snapshot?.cache_hit_ratio ?? (cacheTotal > 0 ? (snapshot?.cache_hit_tokens ?? 0) / cacheTotal : undefined);

  useEffect(() => {
    if (popoverSuppressed && open) setOpen(false);
  }, [popoverSuppressed, open]);

  useEffect(() => {
    if (overlayCloseSignal > lastOverlayCloseSignal.current) {
      lastOverlayCloseSignal.current = overlayCloseSignal;
      setOpen(false);
    }
  }, [overlayCloseSignal]);

  useEffect(() => {
    if (open === lastReportedOpen.current) return;
    lastReportedOpen.current = open;
    onPopoverOpenChange?.(open);
  }, [open, onPopoverOpenChange]);

  const showPopover = open && !popoverSuppressed;

  return (
    <div className="relative flex h-8 w-8 shrink-0 items-center justify-center">
      <HoverCard open={showPopover} onOpenChange={(next, details) => {
        if (details.reason === "trigger-press") details.cancel();
        else if (!popoverSuppressed) setOpen(next);
      }}>
      <HoverCardTrigger delay={120} closeDelay={150} render={<button
        type="button"
        className={`flex h-8 w-8 items-center justify-center rounded-full text-text-secondary ${MOTION.fast}`}
        aria-label={`Context ${percent}%`}
        onClick={() => {
          if (!popoverSuppressed) setOpen((value) => !value);
        }}
      >
        <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
          <circle
            cx="10"
            cy="10"
            r="8.5"
            fill="none"
            className="stroke-border-theme"
            strokeWidth="1.8"
          />
          {(ratio > 0 || !isEmptySession) && (
            <circle
              cx="10"
              cy="10"
              r="8.5"
              fill="none"
              stroke={stroke}
              strokeWidth="1.8"
              strokeLinecap="round"
              strokeDasharray={circumference}
              strokeDashoffset={dashOffset}
              transform="rotate(-90 10 10)"
            />
          )}
        </svg>
      </button>} />

      <HoverCardContent side="top" align="end" className="w-[200px] px-3.5 py-3">
          <div className="flex items-baseline justify-between gap-2">
            <span className="font-semibold">上下文</span>
            <span className="text-[18px] font-semibold leading-none">{percent}%</span>
          </div>
          <div className="mt-1.5 text-[13px] text-text-secondary">
            {hasWindow
              ? `${formatTokens(usedTokens)} / ${formatTokens(contextWindow)}`
              : formatTokens(usedTokens)}
          </div>

          {blocks.length > 0 && (
            <div className="mt-2 space-y-1.5 border-t border-border-theme pt-2">
              {blocks.map((block) => (
                <div key={`${block.kind}-${block.source}`} className="flex items-center justify-between gap-2">
                  <span className="min-w-0 truncate text-text-secondary">{block.name}</span>
                  <span className="shrink-0 font-medium">{formatTokens(block.tokens)}</span>
                </div>
              ))}
            </div>
          )}

          {(cacheRatio != null || snapshot?.cache_hit_tokens) && (
            <div className="mt-2 flex items-center justify-between border-t border-border-theme pt-2 text-text-secondary">
              <span>缓存</span>
              <span className="font-medium text-text-base">
                {cacheRatio == null ? formatTokens(snapshot?.cache_hit_tokens ?? 0) : `${Math.round(cacheRatio * 100)}%`}
              </span>
            </div>
          )}
      </HoverCardContent>
      </HoverCard>
    </div>
  );
}
