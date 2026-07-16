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
};
