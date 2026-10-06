import type {
  ChatMessage,
  ComposerAttachment,
  ComposerMention,
  ComposerSkillSelection,
  TokenUsage,
  ToolCall,
} from "../../types";

export type ChatBlock =
  | {
      kind: "user";
      id: string;
      text: string;
      attachments: ComposerAttachment[];
      selectedSkills: ComposerSkillSelection[];
      mentions: ComposerMention[];
      source: ChatMessage;
    }
  | {
      kind: "assistant";
      id: string;
      text: string;
      tone?: "normal" | "error";
      usage?: TokenUsage;
      runMs?: number;
      source: ChatMessage;
    }
  | {
      kind: "reasoning";
      id: string;
      text: string;
    }
  | {
      kind: "tool";
      id: string;
      tool: ToolCall;
    }
  | {
      kind: "compaction";
      id: string;
      tokensBefore: number;
      tokensAfter: number;
      strategy: string;
      summary: string;
    }
  | {
      kind: "memories";
      id: string;
      count: number;
      /** Which entries came in; the row shows them, not just the count. */
      ids: string[];
      latencyMs: number;
    };

export type Turn = {
  user?: Extract<ChatBlock, { kind: "user" }>;
  blocks: ChatBlock[];
  /** Turn-level metrics, lifted off the blocks. They describe the whole turn,
   * so they must not ride on whichever text block currently counts as the
   * answer: that block moves between the answer surface and the process
   * timeline every time the model starts another tool call. */
  usage?: TokenUsage;
  runMs?: number;
};

export type ProcessSection = {
  id: string;
  kind: "reasoning" | "execution" | "output";
  blocks: ChatBlock[];
};

export type TurnSections = {
  processBlocks: ChatBlock[];
  assistantContentBlocks: Extract<ChatBlock, { kind: "assistant" }>[];
  /** Session-level markers (compaction boundary, injected memories). They are
   * neither process steps nor an answer, so they render as their own quiet rows. */
  markerBlocks: Extract<ChatBlock, { kind: "compaction" | "memories" }>[];
};
