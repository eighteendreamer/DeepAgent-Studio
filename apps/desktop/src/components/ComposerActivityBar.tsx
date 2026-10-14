import { useRef, useState } from "react";
import { CheckCircle2, ChevronUp, Circle, FileCode2, GitCompareArrows, ListTodo, LoaderCircle, X } from "lucide-react";
import { Button } from "./shadcn/button";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from "./shadcn/item";
import { Popover, PopoverClose, PopoverContent, PopoverTitle, PopoverTrigger } from "./shadcn/popover";
import { cn } from "./shadcn/utils";

export interface ComposerTodo {
  id: string;
  text: string;
  status: "pending" | "in_progress" | "completed";
}

export interface ComposerFileChange {
  path: string;
  additions: number;
  deletions: number;
}

interface Props {
  todos: ComposerTodo[];
  changes: ComposerFileChange[];
}

function PanelHeading({ title, summary }: { title: string; summary: string }) {
  return (
    <div className="flex items-center justify-between gap-3 border-b border-border-theme px-4 py-3">
      <div className="flex items-center gap-2.5">
        <PopoverTitle className="text-[13px] font-medium">{title}</PopoverTitle>
        <span className="text-[11px] text-text-secondary">{summary}</span>
      </div>
      <PopoverClose render={<Button variant="ghost" size="icon" className="h-6 w-6" aria-label="收起列表" />}>
        <X size={13} />
      </PopoverClose>
    </div>
  );
}

const triggerClass = "h-8 gap-1.5 rounded-none px-2 text-[11px] font-normal text-text-secondary hover:bg-hover-bg data-[popup-open]:bg-hover-bg data-[popup-open]:text-text-base sm:gap-2 sm:px-3 sm:text-[12px]";

/** Presentation only: data is supplied by the chat view, never fetched here. */
export function ComposerActivityBar({ todos, changes }: Props) {
  const barRef = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<"todos" | "changes" | null>(null);
  const completed = todos.filter((todo) => todo.status === "completed").length;
  const additions = changes.reduce((sum, file) => sum + file.additions, 0);
  const deletions = changes.reduce((sum, file) => sum + file.deletions, 0);
  if (!todos.length && !changes.length) return null;

  return (
    <div className="mb-2.5 flex justify-center">
      <div ref={barRef} role="group" aria-label="任务进度与文件变更" className="flex max-w-full items-center overflow-hidden rounded-full border border-border-theme bg-elevated-bg shadow-[0_2px_8px_rgb(0,0,0,0.035)]">
        <Popover open={active === "todos"} onOpenChange={(open) => setActive((current) => open ? "todos" : current === "todos" ? null : current)}>
          <PopoverTrigger render={<Button variant="ghost" className={triggerClass} />} aria-label={`待办，已完成 ${completed} 项，共 ${todos.length} 项`}>
            <ListTodo size={13} className="hidden sm:block" />
            <span>待办 <span className="ml-1 tabular-nums">{completed}/{todos.length}</span></span>
            <ChevronUp size={12} className={cn("hidden opacity-60 transition-transform sm:block", active === "todos" && "rotate-180")} />
          </PopoverTrigger>
          <PopoverContent side="top" anchor={barRef} className="w-[460px] max-w-[calc(100vw-32px)] overflow-hidden">
            <PanelHeading title="待办清单" summary={`${completed}/${todos.length} 已完成`} />
            <div className="max-h-[min(340px,calc(100dvh-280px))] overflow-y-auto overscroll-contain p-1.5">
              <ItemGroup aria-label="待办清单">
                {todos.map((todo) => (
                  <Item key={todo.id} size="sm" variant={todo.status === "in_progress" ? "muted" : "default"}>
                    <ItemMedia className="self-start pt-0.5">
                      {todo.status === "completed" ? <CheckCircle2 size={15} className="text-green-600" />
                        : todo.status === "in_progress" ? <LoaderCircle size={15} className="animate-spin text-blue-500" />
                        : <Circle size={15} className="text-text-secondary/45" />}
                    </ItemMedia>
                    <ItemContent>
                      <ItemTitle className={cn("font-normal", todo.status === "completed" && "text-text-secondary line-through decoration-text-secondary/40")}>{todo.text}</ItemTitle>
                    </ItemContent>
                    {todo.status === "in_progress" && <ItemActions className="text-[10px] text-blue-500">进行中</ItemActions>}
                  </Item>
                ))}
                {!todos.length && <Item size="sm"><ItemContent><ItemTitle className="font-normal text-text-secondary">暂无待办</ItemTitle></ItemContent></Item>}
              </ItemGroup>
            </div>
          </PopoverContent>
        </Popover>
        <div className="h-4 w-px bg-border-theme" aria-hidden="true" />
        <Popover open={active === "changes"} onOpenChange={(open) => setActive((current) => open ? "changes" : current === "changes" ? null : current)}>
          <PopoverTrigger render={<Button variant="ghost" className={triggerClass} />} aria-label={`变更，${changes.length} 个文件，新增 ${additions} 行，删除 ${deletions} 行`}>
            <GitCompareArrows size={13} className="hidden sm:block" />
            <span className="whitespace-nowrap">{changes.length} 个文件变更</span>
            <span className="tabular-nums text-green-600">+{additions}</span>
            <span className="tabular-nums text-red-500">−{deletions}</span>
            <ChevronUp size={12} className={cn("hidden opacity-60 transition-transform sm:block", active === "changes" && "rotate-180")} />
          </PopoverTrigger>
          <PopoverContent side="top" anchor={barRef} className="w-[460px] max-w-[calc(100vw-32px)] overflow-hidden">
            <PanelHeading title="文件变更" summary={`${changes.length} 个文件`} />
            <div className="max-h-[min(340px,calc(100dvh-280px))] overflow-y-auto overscroll-contain p-1.5">
              <ItemGroup aria-label="文件变更">
                {changes.map((file) => (
                  <Item key={file.path} size="sm">
                    <ItemMedia><FileCode2 size={16} className="text-text-secondary" /></ItemMedia>
                    <ItemContent>
                      <ItemTitle className="truncate font-normal">{file.path.split(/[\\/]/).pop()}</ItemTitle>
                      <ItemDescription className="truncate" title={file.path}>{file.path}</ItemDescription>
                    </ItemContent>
                    <ItemActions className="gap-3 text-[12px] tabular-nums">
                      <span className="text-green-600">+{file.additions}</span>
                      <span className="text-red-500">−{file.deletions}</span>
                    </ItemActions>
                  </Item>
                ))}
                {!changes.length && <Item size="sm"><ItemContent><ItemTitle className="font-normal text-text-secondary">暂无文件变更</ItemTitle></ItemContent></Item>}
              </ItemGroup>
            </div>
          </PopoverContent>
        </Popover>
      </div>
    </div>
  );
}
