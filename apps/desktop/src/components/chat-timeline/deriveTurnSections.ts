import type { ChatBlock, ProcessSection, Turn, TurnSections } from "./timelineTypes";

/**
 * Index of the turn's final-answer block: the last assistant block with visible
 * content, but ONLY when no tool call appears after it. Text emitted before or
 * between tool calls (preambles / narration) is NOT the final answer — it stays
 * in the process timeline. The trailing block (nothing after it) is always the
 * live-streaming answer, so it renders OUTSIDE the collapsed timeline and streams
 * token-by-token during a run instead of being hidden inside a collapsed
 * "中间输出" process section (which only auto-expands for reasoning).
 *
 * This satisfies both cases at once:
 *  - a preamble followed by tool calls → stays inside the process timeline
 *    (never shown as a fake final answer while the model is still working),
 *  - the actively-streaming tail (nothing after it) → renders outside and
 *    streams live, whether it is an early preamble or the real final answer.
 */
function findFinalAssistantContentIndex(blocks: ChatBlock[]): number {
  let lastContent = -1;
  for (let index = blocks.length - 1; index >= 0; index -= 1) {
    const block = blocks[index];
    if (block.kind === "assistant" && block.text.trim()) {
      lastContent = index;
      break;
    }
  }
  if (lastContent === -1) return -1;
  // A tool call after the last assistant text means the model is still working
  // (or the turn ended on a tool) — there is no final answer to surface yet.
  for (let index = lastContent + 1; index < blocks.length; index += 1) {
    const block = blocks[index];
    if (block.kind === "tool" && block.tool.meta?.isHook !== true) return -1;
  }
  return lastContent;
}

export function deriveTurnSections(turn: Turn, _isProcessing: boolean): TurnSections {
  const processBlocks: ChatBlock[] = [];
  const assistantContentBlocks: Extract<ChatBlock, { kind: "assistant" }>[] = [];
  const finalAssistantContentIndex = findFinalAssistantContentIndex(turn.blocks);

  turn.blocks.forEach((block, index) => {
    if (block.kind === "assistant") {
      if (index === finalAssistantContentIndex) {
        assistantContentBlocks.push(block);
      } else {
        processBlocks.push(block);
      }
      return;
    }
    processBlocks.push(block);
  });

  return { processBlocks, assistantContentBlocks };
}

export function groupProcessSections(blocks: ChatBlock[]): ProcessSection[] {
  const sections: ProcessSection[] = [];
  for (const block of blocks) {
    const kind: ProcessSection["kind"] =
      block.kind === "reasoning" ? "reasoning" : block.kind === "assistant" ? "output" : "execution";
    const last = sections[sections.length - 1];
    if (last && last.kind === kind) {
      last.blocks.push(block);
      continue;
    }
    sections.push({ id: `${kind}-${block.id}`, kind, blocks: [block] });
  }
  return sections;
}
