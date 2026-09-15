import { create } from "zustand";
import type { WorkflowEdge, WorkflowNode } from "../types";
import {
  captureWorkflowFragment, parseSnippetLibrary, SNIPPET_LIMITS, validateSnippetName,
  type SnippetLibrary, type WorkflowSnippet,
} from "../utils/workflowSnippets";

export const SNIPPET_STORAGE_KEY = "workflow-canvas-professional-snippets";

function readLibrary(): SnippetLibrary {
  let raw: string | null;
  try { raw = localStorage.getItem(SNIPPET_STORAGE_KEY); }
  catch { throw new Error("无法读取本地片段库，请检查存储权限后重试；未修改已有数据。"); }
  return parseSnippetLibrary(raw);
}

function writeLibrary(library: SnippetLibrary): WorkflowSnippet[] {
  const raw = JSON.stringify(library);
  // Validate the complete next library before writing; never repair unknown data by dropping it.
  const validated = parseSnippetLibrary(raw);
  try { localStorage.setItem(SNIPPET_STORAGE_KEY, raw); }
  catch { throw new Error("片段库写入失败，可能是存储空间不足或存储被禁用；本次更改未保存。"); }
  return validated.snippets;
}

interface SnippetState {
  snippets: WorkflowSnippet[];
  loaded: boolean;
  error: string | null;
  load: () => boolean;
  saveSnippet: (name: string, nodes: WorkflowNode[], edges: WorkflowEdge[]) => void;
  deleteSnippet: (id: string) => void;
}

export const useSnippetStore = create<SnippetState>((set) => ({
  snippets: [], loaded: false, error: null,
  load: () => {
    try {
      set({ snippets: readLibrary().snippets, loaded: true, error: null });
      return true;
    } catch (error) {
      set({ loaded: true, error: error instanceof Error ? error.message : "片段库读取失败，未修改已有数据。" });
      return false;
    }
  },
  saveSnippet: (rawName, nodes, edges) => {
    const name = validateSnippetName(rawName);
    // Re-read for every mutation so another window's data (including invalid data) is not overwritten.
    const library = readLibrary();
    if (library.snippets.some((snippet) => snippet.name.toLowerCase() === name.toLowerCase())) {
      throw new Error("已存在同名片段，请使用其他名称；不会覆盖原片段。");
    }
    if (library.snippets.length >= SNIPPET_LIMITS.items) throw new Error("片段库已达 50 项，请先删除不再需要的片段。");
    const fragment = captureWorkflowFragment(nodes, edges);
    const base = `snippet-${crypto.randomUUID()}`;
    let id = base; let suffix = 0;
    while (library.snippets.some((snippet) => snippet.id === id)) id = `${base}-${++suffix}`;
    const snippet: WorkflowSnippet = { id, name, createdAt: Date.now(), ...fragment };
    const snippets = writeLibrary({ version: 1, snippets: [...library.snippets, snippet] });
    set({ snippets, loaded: true, error: null });
  },
  deleteSnippet: (id) => {
    const library = readLibrary();
    if (!library.snippets.some((snippet) => snippet.id === id)) throw new Error("该片段已不存在，请刷新片段库。");
    const snippets = writeLibrary({ version: 1, snippets: library.snippets.filter((snippet) => snippet.id !== id) });
    set({ snippets, loaded: true, error: null });
  },
}));
