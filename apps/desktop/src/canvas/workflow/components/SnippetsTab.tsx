import { useEffect, useId, useState } from "react";
import { Layers, MoreHorizontal, Plus, Save, Trash2 } from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../../../components/shadcn/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "../../../components/shadcn/dropdown-menu";
import { message } from "../../../components/message";
import { useProfessionalStore } from "../store/professionalStore";
import { useSnippetStore } from "../store/snippetStore";
import { onCanvasPreferenceChanged } from "../utils/canvasPreferences";
import { instantiateWorkflowFragment, SNIPPET_LIMITS, SNIPPET_SAFETY_NOTICE, type WorkflowSnippet } from "../utils/workflowSnippets";
import { CANVAS_BUTTON_CLASS, CANVAS_MENU_CLASS, CanvasField, CanvasInput } from "./CanvasFields";

export function SnippetsTab({ worldX, worldY, onInserted }: { worldX: number; worldY: number; onInserted: () => void }) {
  const snippets = useSnippetStore((state) => state.snippets);
  const error = useSnippetStore((state) => state.error);
  const loaded = useSnippetStore((state) => state.loaded);
  const selectedCount = useProfessionalStore((state) => state.nodes.filter((node) => node.selected && node.type?.startsWith("professional-")).length);
  const [name, setName] = useState("");
  const [query, setQuery] = useState("");
  const [confirmation, setConfirmation] = useState<{ action: "insert" | "delete"; snippet: WorkflowSnippet } | null>(null);
  const nameId = useId();
  const titleId = useId();
  const descriptionId = useId();

  const reload = () => {
    const store = useSnippetStore.getState();
    if (!store.load()) message.error(useSnippetStore.getState().error ?? "片段库读取失败");
  };
  useEffect(() => {
    reload();
    return onCanvasPreferenceChanged((key) => {
      // 载入完成或其他画布窗口改动库时重新读取。
      if (key === "workflow-snippets") reload();
    });
  }, []);

  const save = () => {
    try {
      const graph = useProfessionalStore.getState();
      useSnippetStore.getState().saveSnippet(name, graph.nodes, graph.edges);
      setName("");
      message.success("片段已保存到内核数据库；密码与敏感请求头已排除，使用前请重新配置。");
    } catch (failure) { message.error(failure instanceof Error ? failure.message : "片段保存失败"); }
  };

  const confirm = () => {
    if (!confirmation) return;
    try {
      if (confirmation.action === "delete") {
        useSnippetStore.getState().deleteSnippet(confirmation.snippet.id);
        message.success("片段已删除；画布上的节点不受影响。");
      } else {
        // Refresh before using the item; a stale dialog must not resurrect deleted/invalid data.
        const library = useSnippetStore.getState();
        if (!library.load()) throw new Error(useSnippetStore.getState().error ?? "片段库读取失败");
        const snippet = useSnippetStore.getState().snippets.find((item) => item.id === confirmation.snippet.id);
        if (!snippet) throw new Error("该片段已不存在，请刷新后重试。");
        const graph = useProfessionalStore.getState();
        const fragment = instantiateWorkflowFragment({ nodes: snippet.nodes, edges: snippet.edges }, worldX, worldY, graph.nodes, graph.edges);
        graph.insertFragment(fragment);
        message.success("片段已插入，可一次撤销；请检查变量和凭据配置后运行。");
        onInserted();
      }
      setConfirmation(null);
    } catch (failure) { message.error(failure instanceof Error ? failure.message : "片段操作失败"); }
  };

  const visible = snippets.filter((snippet) => snippet.name.toLowerCase().includes(query.trim().toLowerCase()));
  return (
    <section aria-label="本地工作流片段库" className="space-y-3 p-2 text-white/80">
      <div className="space-y-2 border-b border-white/10 pb-3">
        <CanvasField label="保存选区为片段" htmlFor={nameId}>
          <CanvasInput id={nameId} value={name} onChange={(event) => setName(event.target.value)} placeholder="输入片段名称" maxLength={SNIPPET_LIMITS.name} onKeyDown={(event) => { event.stopPropagation(); if (event.key === "Enter" && selectedCount && name.trim() && loaded && !error) save(); }} />
        </CanvasField>
        <Button size="sm" variant="ghost" className={`${CANVAS_BUTTON_CLASS} w-full`} disabled={!selectedCount || !name.trim() || !loaded || Boolean(error)} onClick={save}>
          <Save className="h-3.5 w-3.5" />保存选中的 {selectedCount} 个节点
        </Button>
        <p className="text-[10px] leading-relaxed text-white/45">{SNIPPET_SAFETY_NOTICE}</p>
      </div>
      {error ? (
        <div role="alert" className="space-y-2 text-xs text-red-300">
          <p className="break-words">{error}</p>
          <p className="text-[10px] text-white/50">原始库未覆盖。修复数据库中的片段文档后重试，不会自动重置片段。</p>
          <Button size="sm" variant="ghost" className={CANVAS_BUTTON_CLASS} onClick={reload}>重新读取</Button>
        </div>
      ) : (
        <>
          <div className="flex items-center justify-between text-[10px] text-white/45"><span>本机片段 · {snippets.length}/{SNIPPET_LIMITS.items}</span><span>插入后独立编辑</span></div>
          <CanvasInput aria-label="搜索片段" placeholder="搜索片段名称" value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => event.stopPropagation()} />
          {!loaded ? <p role="status" className="py-4 text-center text-xs text-white/45">正在读取片段库</p> : visible.length ? (
            <div className="space-y-1">
              {visible.map((snippet) => (
                <div key={snippet.id} className="flex items-center gap-2 rounded-lg border border-white/[0.06] p-2">
                  <Layers className="h-4 w-4 shrink-0 text-white/40" />
                  <div className="min-w-0 flex-1"><p className="truncate text-xs" title={snippet.name}>{snippet.name}</p><p className="mt-1 text-[10px] text-white/40">{snippet.nodes.length} 个节点 · {snippet.edges.length} 条连线</p></div>
                  <Button size="icon" variant="ghost" className={`${CANVAS_BUTTON_CLASS} !h-7 !w-7`} aria-label={`插入片段 ${snippet.name}`} onClick={() => setConfirmation({ action: "insert", snippet })}><Plus className="h-3.5 w-3.5" /></Button>
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild><Button size="icon" variant="ghost" className={`${CANVAS_BUTTON_CLASS} !h-7 !w-7`} aria-label={`管理片段 ${snippet.name}`}><MoreHorizontal className="h-3.5 w-3.5" /></Button></DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className={`wf-floating-layer ${CANVAS_MENU_CLASS}`}>
                      <DropdownMenuItem className="!text-red-300 data-[highlighted]:!bg-white/10" onSelect={() => setConfirmation({ action: "delete", snippet })}><Trash2 className="mr-2 h-3.5 w-3.5" />删除片段</DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                </div>
              ))}
            </div>
          ) : (
            <div className="space-y-2 py-5 text-center">
              <Layers className="mx-auto h-5 w-5 text-white/30" />
              <p className="text-xs text-white/65">{snippets.length ? "没有匹配的片段" : "还没有工作流片段"}</p>
              <p className="text-[11px] leading-relaxed text-white/40">{snippets.length ? "换个名称搜索，或清空搜索词。" : "先在专业画布中多选一组相连节点，再打开 Snippets 命名保存。引用的节点和分组需一并选中。"}</p>
            </div>
          )}
        </>
      )}
      <Dialog open={confirmation !== null} onOpenChange={(open) => { if (!open) setConfirmation(null); }}>
        <DialogContent zIndexClass="z-[21000]" className="wf-floating-layer w-[min(420px,calc(100vw-32px))] rounded-2xl border border-white/10 bg-[#1c1d20] p-5 text-white" aria-labelledby={titleId} aria-describedby={descriptionId}>
          <DialogHeader><DialogTitle id={titleId}>{confirmation?.action === "delete" ? "删除片段？" : "插入片段？"}</DialogTitle></DialogHeader>
          <DialogDescription id={descriptionId} className="my-3 break-words !text-white/60">
            {confirmation?.action === "delete"
              ? `将从本机片段库删除「${confirmation.snippet.name}」。此操作不可撤销，但不会删除画布中的节点。`
              : `将「${confirmation?.snippet.name ?? ""}」插入当前选定位置，可一次撤销。密码和敏感请求头需重新配置；片段中的其他参数请自行检查。`}
          </DialogDescription>
          <DialogFooter>
            <Button size="sm" variant="ghost" className={CANVAS_BUTTON_CLASS} onClick={() => setConfirmation(null)}>取消</Button>
            <Button size="sm" variant={confirmation?.action === "delete" ? "destructive" : "ghost"} className={confirmation?.action === "insert" ? CANVAS_BUTTON_CLASS : undefined} onClick={confirm}>{confirmation?.action === "delete" ? "确认删除" : "确认插入"}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
