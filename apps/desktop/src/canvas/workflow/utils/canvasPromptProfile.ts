import { invoke } from "@tauri-apps/api/core";
import type { CreativeNodeData, CreativeNodeKind } from "../types";

/**
 * Prompt-profile binding for creative nodes.
 *
 * A node owns only the reference (`promptProfileId` + version) and its own user
 * text; the system prompt and skill blocks are resolved by the backend, so a
 * prompt can never drift onto the wrong node kind.
 */
export interface CanvasPromptResolution {
  nodeKind: string;
  promptProfileId: string;
  promptProfileVersion: number;
  systemPrompt: string;
  resolvedUserPrompt: string;
  isDefaultUserPrompt: boolean;
  executorKind: string;
  allowedOperations: string[];
  allowedSkillIds: string[];
}

/** 创作节点里承载用户提示词的字段（不同节点字段名不同）。 */
const PROMPT_FIELD: Partial<Record<CreativeNodeKind, "prompt" | "imagePrompt" | "videoPrompt">> = {
  "text-gen": "prompt",
  "script-gen": "prompt",
  director: "prompt",
  "creative-template": "prompt",
  "storyboard-grid": "prompt",
  "character-face": "prompt",
  "character-body": "prompt",
  "character-style": "prompt",
  audio: "prompt",
  "image-gen": "imagePrompt",
  "image-edit": "imagePrompt",
  "video-gen": "videoPrompt",
};

const cache = new Map<string, CanvasPromptResolution | null>();
const pending = new Map<string, Promise<CanvasPromptResolution | null>>();

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** 向内核请求该节点种类唯一对应的提示词档案（按种类缓存）。 */
export function resolveCanvasPromptProfile(
  nodeKind: CreativeNodeKind,
): Promise<CanvasPromptResolution | null> {
  if (!isTauriRuntime()) return Promise.resolve(null);
  if (cache.has(nodeKind)) return Promise.resolve(cache.get(nodeKind) ?? null);
  const inflight = pending.get(nodeKind);
  if (inflight) return inflight;
  const request = invoke<CanvasPromptResolution>("canvas_prompt_resolve", {
    nodeKind,
    userPromptMode: null,
    userPromptOverride: null,
  })
    .then((resolution) => {
      cache.set(nodeKind, resolution);
      return resolution;
    })
    .catch((error: unknown) => {
      console.error(`[canvas] 解析 ${nodeKind} 提示词档案失败:`, error);
      cache.set(nodeKind, null);
      return null;
    })
    .finally(() => {
      pending.delete(nodeKind);
    });
  pending.set(nodeKind, request);
  return request;
}

/**
 * Build the node-data patch that binds a node to its prompt profile.
 *
 * The default user prompt is only filled in while the field is still empty, so
 * a prompt the developer already edited is never overwritten.
 */
export async function canvasPromptProfilePatch(
  kind: CreativeNodeKind,
  current: CreativeNodeData,
): Promise<Partial<CreativeNodeData> | null> {
  const resolution = await resolveCanvasPromptProfile(kind);
  if (!resolution) return null;
  const patch: Partial<CreativeNodeData> = {
    promptProfileId: resolution.promptProfileId,
    promptProfileVersion: resolution.promptProfileVersion,
  };
  const field = PROMPT_FIELD[kind];
  if (field) {
    const existing = current[field];
    if (typeof existing === "string" && existing.trim() === "") {
      patch[field] = resolution.resolvedUserPrompt;
    }
  }
  return patch;
}
