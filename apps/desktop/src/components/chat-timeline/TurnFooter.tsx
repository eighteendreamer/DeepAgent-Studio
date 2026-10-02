import { HoverInfo } from "../ui/HoverInfo";
import { useState } from "react";
import { ArrowDown, ArrowUp, Check, CircleX, Clock, Coins, Copy, GitBranch, Hash, Zap } from "lucide-react";
import type { TokenUsage } from "../../types";
import { cnySymbol, formatCny, formatMs, formatTokens } from "./format";

export function TurnFooter({
  usage,
  totalMs,
  answer,
}: {
  usage?: TokenUsage;
  totalMs?: number;
  answer: string;
}) {
  const [copied, setCopied] = useState(false);
  const durationMs = totalMs ?? 0;
  const hasMetrics = Boolean(usage) || durationMs > 0;

  const copyAnswer = () => {
    if (!answer) return;
    navigator.clipboard?.writeText(answer).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      },
      () => {},
    );
  };

  if (!hasMetrics && !answer) return null;

  return (
    <div className="mt-2 flex min-h-7 items-center justify-between gap-3 text-[11.5px] text-text-secondary opacity-80 transition group-hover/message:opacity-100">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 tabular-nums">
        {usage && (
          <>
            <span className="font-semibold text-text-base">
              <Hash size={12} className="mr-0.5 inline" />
              {formatTokens(usage.totalTokens)} tokens
            </span>
            <span className="text-text-tertiary">
              <ArrowDown size={12} className="mr-0.5 inline" />
              输入 {formatTokens(usage.promptTokens)}
            </span>
            <span className="text-text-tertiary">
              <ArrowUp size={12} className="mr-0.5 inline" />
              输出 {formatTokens(usage.completionTokens)}
            </span>
            {usage.cacheHitTokens > 0 && (
              <span className="font-medium text-green-600">
                <Zap size={12} className="mr-0.5 inline" />
                缓存命中 {formatTokens(usage.cacheHitTokens)}
              </span>
            )}
            {usage.cacheMissTokens > 0 && (
              <span className="text-text-tertiary">
                <CircleX size={12} className="mr-0.5 inline" />
                缓存未命中 {formatTokens(usage.cacheMissTokens)}
              </span>
            )}
            <span>
              <Coins size={12} className="mr-0.5 inline" />
              {typeof usage.costYuan === "number" ? formatCny(usage.costYuan) : `${cnySymbol}--`}
            </span>
          </>
        )}
        {durationMs > 0 && (
          <span>
            <Clock size={12} className="mr-1 inline" />
            总耗时: {formatMs(durationMs)}
          </span>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <HoverInfo content="复制回答"><button
          type="button"
          onClick={copyAnswer}
          className="flex h-7 w-7 items-center justify-center rounded-md transition hover:bg-black/5 hover:text-text-base"

          aria-label="复制回答"
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
        </button></HoverInfo>
        <HoverInfo content="从这里创建分支"><button
          type="button"
          className="flex h-7 w-7 items-center justify-center rounded-md transition hover:bg-black/5 hover:text-text-base"

          aria-label="从这里创建分支"
        >
          <GitBranch size={12} />
        </button></HoverInfo>
      </div>
    </div>
  );
}
