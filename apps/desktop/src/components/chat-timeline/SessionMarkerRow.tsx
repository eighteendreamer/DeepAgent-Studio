import { useState } from "react";
import { Bookmark, ChevronRight, Minimize2 } from "lucide-react";
import { Button } from "../shadcn/button";
import { formatMs, formatTokens } from "./format";
import type { ChatBlock } from "./timelineTypes";

type MarkerBlock = Extract<ChatBlock, { kind: "compaction" | "memories" }>;

/**
 * How a boundary came to be, said in the user's terms. The key set is pinned to
 * `deepagent-core/src/compaction_strategy.rs` by
 * `scripts/check-timeline-memory.test.cjs`; a strategy this map does not know is
 * shown verbatim instead of being relabelled as one it does.
 */
const STRATEGY_LABELS: Record<string, string> = {
  session_memory: "按会话笔记",
  model: "模型总结",
  manual: "手动压缩",
  resume: "恢复会话时压缩",
  proactive_threshold: "超阈值自动",
  prefire_pass2: "复用预压缩",
  reactive_model: "溢出后模型总结",
  micro_compact: "清理工具输出",
};

/**
 * One quiet line for a session-level marker, rendered outside the collapsed
 * process list so it survives scrolling back through a long session. The
 * compaction line can open the summary text it replaced, which is the only way
 * to check that compression kept the thread instead of eating it.
 */
export function SessionMarkerRow({ block }: { block: MarkerBlock }) {
  const [open, setOpen] = useState(false);

  if (block.kind === "compaction") {
    const saved = Math.max(0, block.tokensBefore - block.tokensAfter);
    const strategy = STRATEGY_LABELS[block.strategy] ?? block.strategy;
    const summary = block.summary.trim();
    return (
      <div className="text-[13px] text-text-secondary">
        <div className="flex items-center gap-1.5">
          <span className="flex h-5 w-5 items-center justify-center">
            <Minimize2 size={13} />
          </span>
          <span>已压缩较早的对话</span>
          <span className="text-[12px]">{`${formatTokens(block.tokensBefore)} → ${formatTokens(block.tokensAfter)}`}</span>
          {saved > 0 && <span className="text-[12px]">省出 {formatTokens(saved)} 上下文</span>}
          <span className="text-[12px]">{strategy}</span>
          {summary && (
            <Button
              variant="ghost"
              size="sm"
              className="h-5 gap-1 px-1 text-[12px]"
              onClick={() => setOpen((value) => !value)}
            >
              <ChevronRight
                size={12}
                className={open ? "rotate-90 transition-transform" : "transition-transform"}
              />
              {open ? "收起摘要" : "查看摘要"}
            </Button>
          )}
        </div>
        {open && (
          <pre className="mt-1 max-h-60 overflow-auto whitespace-pre-wrap rounded-lg bg-gray-50 px-3 py-2 font-sans text-[12px] leading-relaxed text-text-base">
            {summary}
          </pre>
        )}
      </div>
    );
  }

  // §7.2 asks for WHICH entries came in, not just how many. The ids are readable
  // slugs (the vault derives the title from them), and they already travel on the
  // durable event and the DTO for exactly this display.
  const ids = block.ids;
  return (
    <div className="text-[13px] text-text-secondary">
      <div className="flex items-center gap-1.5">
        <span className="flex h-5 w-5 items-center justify-center">
          <Bookmark size={13} />
        </span>
        <span>{`已带入 ${block.count} 条相关记忆`}</span>
        {block.latencyMs > 0 && <span className="text-[12px]">{formatMs(block.latencyMs)}</span>}
        {ids.length > 0 && (
          <Button
            variant="ghost"
            size="sm"
            className="h-5 gap-1 px-1 text-[12px]"
            onClick={() => setOpen((value) => !value)}
          >
            <ChevronRight
              size={12}
              className={open ? "rotate-90 transition-transform" : "transition-transform"}
            />
            {open ? "收起条目" : "哪几条"}
          </Button>
        )}
      </div>
      {open && (
        <ul className="mt-1 flex flex-wrap gap-1.5">
          {ids.map((entryId) => (
            <li
              key={entryId}
              className="rounded bg-gray-50 px-1.5 py-0.5 font-mono text-[12px] text-text-base"
            >
              {entryId}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
