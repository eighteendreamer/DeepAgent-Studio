import { useMemo, useState } from "react";
import { Braces, Search } from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from "../../../components/shadcn/dropdown-menu";
import { useProfessionalStore } from "../store/professionalStore";
import { getAvailableVariables } from "../utils/workflowVariables";
import { CANVAS_BUTTON_CLASS, CANVAS_MENU_CLASS, CanvasInput } from "./CanvasFields";

export function VariablePicker({ nodeId, onSelect, types, label = "引用变量" }: {
  nodeId: string;
  onSelect: (reference: string) => void;
  types?: string[];
  label?: string;
}) {
  const nodes = useProfessionalStore((state) => state.nodes);
  const edges = useProfessionalStore((state) => state.edges);
  const [query, setQuery] = useState("");
  const variables = useMemo(() => getAvailableVariables(nodeId, nodes, edges, types), [nodeId, nodes, edges, types]);
  const filtered = variables.filter((variable) => `${variable.nodeLabel} ${variable.name} ${variable.type}`.toLowerCase().includes(query.toLowerCase()));
  return (
    <DropdownMenu onOpenChange={() => setQuery("")}>
      <DropdownMenuTrigger asChild>
        <Button size="sm" variant="ghost" className={CANVAS_BUTTON_CLASS} aria-label={label}><Braces className="h-3.5 w-3.5" />{label}</Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={6} className={`${CANVAS_MENU_CLASS} !w-72`} onPointerDown={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()} onContextMenu={(event) => event.stopPropagation()}>
        <div className="flex items-center gap-1 p-1"><Search className="h-3.5 w-3.5 text-white/40" /><CanvasInput autoFocus aria-label="搜索上游变量" placeholder="搜索节点或变量" value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => event.stopPropagation()} /></div>
        {filtered.map((variable) => (
          <DropdownMenuItem key={variable.reference} onSelect={() => onSelect(variable.reference)} className="!flex !items-start !gap-2 !rounded-lg !px-2.5 !py-2 !text-white/85 data-[highlighted]:!bg-white/10">
            <div className="min-w-0 flex-1"><div className="truncate text-[10px] text-white/40">{variable.nodeLabel}</div><div className="truncate font-mono text-xs">{variable.name}</div></div><span className="text-[10px] text-violet-300">{variable.type}</span>
          </DropdownMenuItem>
        ))}
        {!filtered.length && <div className="p-3 text-xs text-white/40">{variables.length ? "没有匹配变量" : "请先连接具有匹配输出的上游节点"}</div>}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
