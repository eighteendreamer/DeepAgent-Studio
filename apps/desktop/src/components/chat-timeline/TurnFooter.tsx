import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
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
            <span className="font-semibold text-text-base">{formatTokens(usage.totalTokens)} tokens</span>
            <span>
              ({formatTokens(usage.promptTokens)}
              <FontAwesomeIcon icon={["fas", "arrow-down"]} className="mx-0.5 text-[9px]" />
              {formatTokens(usage.completionTokens)}
              <FontAwesomeIcon icon={["fas", "arrow-up"]} className="ml-0.5 text-[9px]" />)
            </span>
            {usage.cacheHitTokens > 0 && (
              <span className="font-medium text-green-600">
                <FontAwesomeIcon icon={["fas", "bolt"]} className="mr-0.5 text-[9px]" />
                命中缓存 {formatTokens(usage.cacheHitTokens)}
              </span>
            )}
            <span>
              <FontAwesomeIcon icon={["fas", "coins"]} className="mr-0.5 text-[9px]" />
              {typeof usage.costYuan === "number" ? formatCny(usage.costYuan) : `${cnySymbol}--`}
            </span>
          </>
        )}
        {durationMs > 0 && (
          <span>
            <FontAwesomeIcon icon={["far", "clock"]} className="mr-1 text-[9px]" />
            总耗时: {formatMs(durationMs)}
          </span>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <button
          type="button"
          onClick={copyAnswer}
          className="flex h-7 w-7 items-center justify-center rounded-md transition hover:bg-gray-100 hover:text-text-base"
          title="复制回答"
          aria-label="复制回答"
        >
          <FontAwesomeIcon icon={copied ? ["fas", "check"] : ["far", "copy"]} className="text-[12px]" />
        </button>
        <button
          type="button"
          className="flex h-7 w-7 items-center justify-center rounded-md transition hover:bg-gray-100 hover:text-text-base"
          title="从这里创建分支"
          aria-label="从这里创建分支"
        >
          <FontAwesomeIcon icon={["fas", "code-branch"]} className="text-[12px]" />
        </button>
      </div>
    </div>
  );
}
