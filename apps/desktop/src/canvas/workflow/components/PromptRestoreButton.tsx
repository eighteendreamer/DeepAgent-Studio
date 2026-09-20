import type { CreativeNodeKind } from "../types";
import { promptFieldOf, useDefaultUserPrompt } from "../utils/canvasPromptProfile";

/**
 * “恢复默认提示词”：只在节点文本已被改写、且该种类在内核有默认用户提示词时出现。
 * 默认值来自内核的提示词档案，前端不复制一份，因此档案升级后按钮仍然还原到最新默认。
 */
export function PromptRestoreButton({
  kind,
  value,
  onUpdate,
}: {
  kind?: CreativeNodeKind;
  value: string;
  onUpdate: (patch: Record<string, unknown>) => void;
}) {
  const defaultPrompt = useDefaultUserPrompt(kind);
  const field = kind ? promptFieldOf(kind) : undefined;
  if (!field || !defaultPrompt.trim()) return null;
  if (value.trim() === defaultPrompt.trim()) return null;

  return (
    <button
      type="button"
      title="恢复为该节点提示词档案的默认内容"
      onClick={() => onUpdate({ [field]: defaultPrompt })}
      className="self-start text-[10px] transition-colors hover:underline"
      style={{ color: "rgba(248,248,248,0.45)" }}
    >
      恢复默认提示词
    </button>
  );
}
