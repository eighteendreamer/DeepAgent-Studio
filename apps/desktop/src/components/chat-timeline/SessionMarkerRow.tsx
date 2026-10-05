import { Bookmark, Minimize2 } from "lucide-react";
import { formatMs, formatTokens } from "./format";
import type { ChatBlock } from "./timelineTypes";

type MarkerBlock = Extract<ChatBlock, { kind: "compaction" | "memories" }>;

/**
 * One quiet line for a session-level marker, rendered outside the collapsed
 * process list so it survives scrolling back through a long session. Mirrors the
 * reference client's boundary row: a single dim line, no card, no numbers beyond
 * what tells the user how much context was reclaimed.
 */
export function SessionMarkerRow({ block }: { block: MarkerBlock }) {
  if (block.kind === "compaction") {
    const saved = Math.max(0, block.tokensBefore - block.tokensAfter);
    return (
      <div className="flex items-center gap-1.5 text-[13px] text-text-secondary">
        <span className="flex h-5 w-5 items-center justify-center">
          <Minimize2 size={13} />
        </span>
        <span>已压缩较早的对话</span>
        <span className="text-[12px]">{`${formatTokens(block.tokensBefore)} → ${formatTokens(block.tokensAfter)}`}</span>
        {saved > 0 && <span className="text-[12px]">省出 {formatTokens(saved)} 上下文</span>}
      </div>
    );
  }

  return (
    <div className="flex items-center gap-1.5 text-[13px] text-text-secondary">
      <span className="flex h-5 w-5 items-center justify-center">
        <Bookmark size={13} />
      </span>
      <span>{`已带入 ${block.count} 条相关记忆`}</span>
      {block.latencyMs > 0 && (
        <span className="text-[12px]">{formatMs(block.latencyMs)}</span>
      )}
    </div>
  );
}
