import type { ChatBlock, Turn } from "./timelineTypes";

export function groupTurns(blocks: ChatBlock[]): Turn[] {
  const turns: Turn[] = [];
  let current: Turn | null = null;

  for (const block of blocks) {
    if (block.kind === "user") {
      if (current) turns.push(current);
      current = { user: block, blocks: [], activity: block.source.turn };
      continue;
    }
    if (!current) current = { blocks: [] };
    current.blocks.push(block);
    if (block.kind === "assistant" && block.source.turn) current.activity = block.source.turn;
    if (block.kind === "assistant") {
      // Last writer wins: the mapper stamps every text segment of the turn with
      // the message's current usage, so the newest one is the live total.
      current.usage = block.usage ?? current.usage;
      current.runMs = block.runMs ?? current.runMs;
      current.editedFiles = block.source.editedFiles ?? current.editedFiles;
    }
  }

  if (current) turns.push(current);
  return turns;
}

export function stableTurnKey(turn: Turn, fallbackIndex: number): string {
  return turn.activity?.id ?? turn.user?.id ?? turn.blocks[0]?.id ?? `turn-${fallbackIndex}`;
}
