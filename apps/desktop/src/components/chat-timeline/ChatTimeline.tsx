import { useMemo } from "react";
import type { ChatMessage, ComposerMention, ComposerSkillSelection } from "../../types";
import { chatMessagesToBlocks } from "./chatMessageMapper";
import { groupTurns, stableTurnKey } from "./groupTurns";
import { MessageTurn } from "./MessageTurn";

export function ChatTimeline({
  messages,
  busy,
  onResend,
  onOpenUrl,
}: {
  messages: ChatMessage[];
  busy: boolean;
  onResend: (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => void;
  onOpenUrl?: (url: string) => void;
}) {
  const turns = useMemo(() => groupTurns(chatMessagesToBlocks(messages)), [messages]);

  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-8">
      {turns.map((turn, index) => (
        <MessageTurn
          key={stableTurnKey(turn, index)}
          turn={turn}
          processing={busy && index === turns.length - 1}
          busy={busy}
          onResend={onResend}
          onOpenUrl={onOpenUrl}
        />
      ))}
    </div>
  );
}
