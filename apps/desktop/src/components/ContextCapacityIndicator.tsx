import { useMemo, useState, useEffect, useRef } from "react";
import type { ContextUsageSnapshot } from "../types";
import { MOTION } from "./ui/motion";

interface Props {
  snapshot?: ContextUsageSnapshot | null;
  modelId?: string;
  fallbackPromptTokens?: number;
  /** Hide popover while another toolbar overlay is open. */
  popoverSuppressed?: boolean;
  /** Increment to force-close the popover. */
  overlayCloseSignal?: number;
  onPopoverOpenChange?: (open: boolean) => void;
}

function contextWindowForModel(modelId?: string): number {
  if (modelId === "deepseek-v4-flash" || modelId === "deepseek-v4-pro") return 1_000_000;
  return 128_000;
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
  modelId,
  fallbackPromptTokens = 0,
  popoverSuppressed = false,
  overlayCloseSignal = 0,
  onPopoverOpenChange,
}: Props) {
  const [open, setOpen] = useState(false);
  const lastOverlayCloseSignal = useRef(overlayCloseSignal);
  const lastReportedOpen = useRef(false);
  const contextWindow = snapshot?.context_window ?? contextWindowForModel(modelId);
  const usedTokens = snapshot?.estimated_prompt_tokens ?? Math.max(0, Math.round(fallbackPromptTokens));
  const isEmptySession = !snapshot && usedTokens === 0;
  const ratio = Math.max(
    0,
    Math.min(1, snapshot?.used_ratio ?? usedTokens / Math.max(1, contextWindow)),
  );
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
    <div
      className="relative flex h-8 w-8 shrink-0 items-center justify-center"
      onMouseEnter={() => {
        if (!popoverSuppressed) setOpen(true);
      }}
      onMouseLeave={() => setOpen(false)}
    >
      <button
        type="button"
        className={`flex h-8 w-8 items-center justify-center rounded-full text-text-secondary ${MOTION.fast}`}
        title={`Context ${percent}%`}
        aria-label={`Context ${percent}%`}
        onClick={() => {
          if (popoverSuppressed) return;
          setOpen((value) => !value);
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
      </button>

      {showPopover && (
        <div className="absolute bottom-full right-0 z-50 mb-2 w-[200px] rounded-2xl bg-elevated-bg px-3.5 py-3 text-[12px] text-text-base shadow-[0_6px_24px_rgba(0,0,0,0.10)]">
          <div className="flex items-baseline justify-between gap-2">
            <span className="font-semibold">上下文</span>
            <span className="text-[18px] font-semibold leading-none">{percent}%</span>
          </div>
          <div className="mt-1.5 text-[13px] text-text-secondary">
            {formatTokens(usedTokens)} / {formatTokens(contextWindow)}
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
        </div>
      )}
    </div>
  );
}
