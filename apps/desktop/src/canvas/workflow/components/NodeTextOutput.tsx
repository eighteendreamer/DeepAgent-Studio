import type { NodeStatus } from "../types";

/** 生成节点的正文只接收输出；输入和引用素材留在编辑面板。 */
export function NodeTextOutput({ output, status, monospace = false }: {
  output?: string;
  status: NodeStatus;
  monospace?: boolean;
}) {
  return (
    <div className={`nopan nowheel max-h-64 min-h-12 overflow-y-auto whitespace-pre-wrap break-words rounded-lg bg-white/[0.025] px-2.5 py-2 text-xs leading-relaxed ${monospace ? "font-mono" : ""} ${output ? "text-white/70" : "text-white/30"}`}>
      {output || (status === "running" ? "正在生成…" : "运行后显示输出")}
    </div>
  );
}
