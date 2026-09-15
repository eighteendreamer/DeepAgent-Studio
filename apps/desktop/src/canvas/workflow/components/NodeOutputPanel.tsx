import { Copy } from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import { message } from "../../../components/message";
import type { ProfessionalNodeData } from "../types";
import { getNodeOutputs } from "../utils/nodeRegistry";
import { variableReference } from "../utils/workflowVariables";
import { CANVAS_BUTTON_CLASS } from "./CanvasFields";

export function NodeOutputPanel({ nodeId, data }: { nodeId: string; data: ProfessionalNodeData }) {
  const outputs = getNodeOutputs(data.kind, data);
  const copy = async (name: string) => {
    try {
      await navigator.clipboard.writeText(variableReference(nodeId, name));
      message.success("变量引用已复制");
    } catch (error) {
      message.error(`复制失败：${String(error)}`);
    }
  };
  return (
    <section aria-label="节点输出" className="mt-4 border-t border-white/10 pt-3">
      <h3 className="mb-2 text-xs font-medium text-white/75">输出变量</h3>
      {outputs.length ? outputs.map((output) => (
        <div key={output.name} className="flex items-center gap-2 py-1">
          <div className="min-w-0 flex-1"><span className="break-all font-mono text-xs text-white/80">{output.name}</span>{output.description && <p className="text-[10px] text-white/35">{output.description}</p>}</div>
          <span className="text-[10px] text-violet-300">{output.type}</span>
          <Button size="icon" variant="ghost" className={`${CANVAS_BUTTON_CLASS} !h-6 !w-6`} aria-label={`复制 ${output.name} 引用`} onClick={() => void copy(output.name)}><Copy className="h-3 w-3" /></Button>
        </div>
      )) : <p className="text-[11px] text-white/35">此节点尚未声明输出</p>}
      {data.errorMessage && <p role="alert" className="mt-2 break-words text-xs text-red-300">{data.errorMessage}</p>}
      {data.result !== undefined && <><h4 className="mt-3 text-[11px] text-white/55">最近执行结果{data.executionTime !== undefined ? ` · ${data.executionTime} ms` : ""}</h4><pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-words text-[11px] text-white/75">{JSON.stringify(data.result, null, 2)}</pre></>}
    </section>
  );
}
