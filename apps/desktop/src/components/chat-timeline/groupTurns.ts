import type { ChatBlock, Turn } from "./timelineTypes";

export function groupTurns(blocks: ChatBlock[]): Turn[] {
  const turns: Turn[] = [];
  let current: Turn | null = null;

  for (const block of blocks) {
    if (block.kind === "user") {
      if (current) turns.push(current);
      current = { user: block, blocks: [] };
      continue;
    }
    if (!current) current = { blocks: [] };
    current.blocks.push(block);
  }

  if (current) turns.push(current);
  return turns;
}

export function stableTurnKey(turn: Turn, fallbackIndex: number): string {
  return turn.user?.id ?? turn.blocks[0]?.id ?? `turn-${fallbackIndex}`;
}
