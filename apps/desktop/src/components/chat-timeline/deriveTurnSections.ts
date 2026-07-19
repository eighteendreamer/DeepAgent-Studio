import type { ChatBlock, ProcessSection, Turn, TurnSections } from "./timelineTypes";

function findLastAssistantContentIndex(blocks: ChatBlock[]): number {
  for (let index = blocks.length - 1; index >= 0; index -= 1) {
    const block = blocks[index];
    if (block.kind === "assistant" && block.text.trim()) return index;
  }
  return -1;
}

export function deriveTurnSections(turn: Turn, isProcessing: boolean): TurnSections {
  const processBlocks: ChatBlock[] = [];
  const assistantContentBlocks: Extract<ChatBlock, { kind: "assistant" }>[] = [];
  // Only the SINGLE last assistant text segment is the final answer bubble
  // (rendered outside the collapsed timeline). Every earlier "先看一下…" preface
  // / intermediate narration stays inside the process timeline. While the turn
  // is still processing, NOTHING is promoted to the final body yet — every
  // assistant segment is part of the live trace (mirrors Kun's
  // derive-turn-sections: `isProcessing ? -1 : findLastAssistantContentIndex`).
  const finalAssistantContentIndex = isProcessing
    ? -1
    : findLastAssistantContentIndex(turn.blocks);

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
