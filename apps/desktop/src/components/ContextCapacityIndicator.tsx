import { HoverCard, HoverCardContent, HoverCardTrigger } from "./shadcn/hover-card";
import { useMemo, useState, useEffect, useRef } from "react";
import { CircularProgressbar, buildStyles } from "react-circular-progressbar";
import { Button } from "./shadcn/button";
import type { ContextUsageSnapshot } from "../types";
import { formatTokens } from "./chat-timeline/format";
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

function capacityTone(ratio: number, isEmptySession: boolean): string {
  if (isEmptySession) return "var(--theme-text-secondary, #6b7280)";
  if (ratio >= 0.92) return "#dc4b41";
  if (ratio >= 0.8) return "#d99015";
  return "var(--theme-text-secondary, #6b7280)";
}

/** Projected next request; legacy snapshots retain their original fallback. */
export function contextOccupancyTokens(snapshot: ContextUsageSnapshot): number {
  return snapshot.projected_request_tokens ?? snapshot.measured_prompt_tokens ?? snapshot.estimated_prompt_tokens;
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
  const usedTokens = snapshot
    ? contextOccupancyTokens(snapshot)
    : Math.max(0, Math.round(fallbackPromptTokens));
  const isEmptySession = !snapshot && usedTokens === 0;
  // Without a denominator there is no meaningful occupancy — show an empty ring
  // rather than dividing by a placeholder.
  const ratio = !hasWindow
    ? 0
    : Math.max(0, Math.min(1, usedTokens / contextWindow));
  const percent = Math.round(ratio * 100);
  const stroke = capacityTone(ratio, isEmptySession);
  const blocks = useMemo(
    () => [...(snapshot?.blocks ?? [])].sort((a, b) => b.tokens - a.tokens),
    [snapshot?.blocks],
  );
  // Legacy snapshots may lack a complete block partition.
  const unattributed = snapshot
    ? Math.max(
        0,
        contextOccupancyTokens(snapshot) -
          blocks.reduce((sum, block) => sum + block.tokens, 0),
      )
    : 0;
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
      <HoverCardTrigger delay={120} closeDelay={150} render={<Button
        variant="ghost"
        size="icon"
        className={`h-8 w-8 rounded-full hover:bg-ui-tint ${MOTION.fast}`}
        aria-label={`Context ${percent}%`}
        onClick={() => {
          if (!popoverSuppressed) setOpen((value) => !value);
        }}
      >
        <span className="block h-4 w-4 shrink-0" aria-hidden="true">
          <CircularProgressbar
            value={ratio}
            maxValue={1}
            strokeWidth={12.5}
            className="block h-full w-full [&_.CircularProgressbar-path]:transition-[stroke-dashoffset] [&_.CircularProgressbar-path]:duration-300 motion-reduce:[&_.CircularProgressbar-path]:transition-none"
            styles={buildStyles({
              pathColor: stroke,
              trailColor: "color-mix(in srgb, var(--theme-text-secondary, #6b7280) 30%, var(--theme-bg, #ffffff))",
              strokeLinecap: "round",
            })}
          />
        </span>
      </Button>} />

      <HoverCardContent motionTone="menu" side="top" align="end" className="w-[240px] px-3.5 py-3">
          <div className="flex items-baseline justify-between gap-2">
            <span className="font-semibold">预计下一次请求</span>
            <span className="text-[18px] font-semibold leading-none">{percent}%</span>
          </div>
          <div className="mt-1.5 text-[13px] text-text-secondary">
            {hasWindow
              ? `${formatTokens(usedTokens)} / ${formatTokens(contextWindow)}`
              : formatTokens(usedTokens)}
          </div>

          {snapshot?.last_request_prompt_tokens != null && (
            <div className="mt-1.5 flex justify-between gap-2 text-[12px] text-text-secondary">
              <span>上一次请求</span><span>{formatTokens(snapshot.last_request_prompt_tokens)}</span>
            </div>
          )}
          {!!snapshot?.compaction_threshold && (
            <div className="mt-1.5 flex justify-between gap-2 text-[12px] text-text-secondary">
              <span>自动压缩阈值</span><span>{formatTokens(snapshot.compaction_threshold)}</span>
            </div>
          )}

          {blocks.length > 0 && (
            <div className="mt-2 space-y-1.5 border-t border-border-theme pt-2">
              <div className="text-[11px] text-text-tertiary">上下文组成（估算）</div>
              {blocks.map((block) => (
                <div key={`${block.kind}-${block.source}`} className="flex items-center justify-between gap-2">
                  <span className="min-w-0 truncate text-text-secondary">{block.name}</span>
                  <span className="shrink-0 font-medium">{formatTokens(block.tokens)}</span>
                </div>
              ))}
              {unattributed > 0 && (
                <div className="flex items-center justify-between gap-2">
                  <span className="min-w-0 truncate text-text-secondary">其他/未归类</span>
                  <span className="shrink-0 font-medium">{formatTokens(unattributed)}</span>
                </div>
              )}
            </div>
          )}

          {(cacheRatio != null || snapshot?.cache_hit_tokens) && (
            <div className="mt-2 flex items-center justify-between border-t border-border-theme pt-2 text-text-secondary">
              <span>上次缓存命中</span>
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
