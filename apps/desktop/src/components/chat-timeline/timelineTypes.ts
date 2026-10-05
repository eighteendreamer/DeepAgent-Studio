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
    }
  | {
      kind: "memories";
      id: string;
      count: number;
      latencyMs: number;
    };

export type Turn = {
  user?: Extract<ChatBlock, { kind: "user" }>;
  blocks: ChatBlock[];
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
