import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileText, Plus, X } from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import { Dialog, DialogCloseIcon, DialogContent, DialogHeader, DialogTitle } from "../../../components/shadcn/dialog";
import { PreviewBody } from "../../../components/plugins/FilePreviewPlugin";
import { importCanvasMediaFile } from "../utils/canvasMedia";
import { CANVAS_BUTTON_CLASS } from "./CanvasFields";

interface DocumentAttachment { reference: string; name: string }
const ACCEPT = ".txt,.md,.csv,.docx,.xlsx,.pptx,.pdf";
const MAX_BYTES = 10 * 1024 * 1024;

function isAttachment(value: unknown): value is DocumentAttachment {
  return value !== null && typeof value === "object"
    && typeof (value as DocumentAttachment).reference === "string"
    && typeof (value as DocumentAttachment).name === "string";
}

function originalBlob(dataUrl: string): Blob {
  const match = /^data:([^;,]*);base64,([A-Za-z0-9+/=]*)$/.exec(dataUrl);
  if (!match) throw new Error("文档预览响应无效");
  const decoded = atob(match[2]);
  if (decoded.length > MAX_BYTES) throw new Error("文档超过 10 MiB");
  return new Blob([Uint8Array.from(decoded, char => char.charCodeAt(0))], { type: match[1] });
}

/** Only artifact references enter the graph; previews are transient original bytes. */
export function DocumentAttachments({ nodeId, value, onChange }: {
  nodeId: string; value: unknown; onChange: (value: unknown[]) => void;
}) {
  const entries: unknown[] = Array.isArray(value) ? value : [];
  const latest = useRef({ value, onChange });
  latest.current = { value, onChange };
  const alive = useRef(true);
  const importing = useRef(false);
  const previewVersion = useRef(0);
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [preview, setPreview] = useState<{ name: string; blob?: Blob; error?: string } | null>(null);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; previewVersion.current += 1; };
  }, []);

  async function upload(files: File[]) {
    if (!files.length || importing.current) return;
    setError("");
    if (!entries.every(isAttachment)) { setError("请先移除无效文档引用，再添加附件"); return; }
    if (entries.length + files.length > 5) { setError("最多选择 5 份文档"); return; }
    for (const file of files) {
      if (!/\.(txt|md|csv|docx|xlsx|pptx|pdf)$/i.test(file.name)) { setError(`不支持的文档格式：${file.name}`); return; }
      if (file.size <= 0 || file.size > MAX_BYTES) { setError(`文档不能为空或超过 10 MiB：${file.name}`); return; }
    }
    importing.current = true; setBusy(true);
    const snapshot = value;
    try {
      const added: DocumentAttachment[] = [];
      for (const file of files) {
        if (!alive.current || latest.current.value !== snapshot) return;
        const reference = await importCanvasMediaFile("document", file);
        added.push({ reference, name: file.name });
      }
      if (alive.current && latest.current.value === snapshot) latest.current.onChange([...entries, ...added]);
    } catch {
      if (alive.current) setError("文档上传失败，本次选择未保存。请重试。");
    } finally {
      importing.current = false;
      if (alive.current) setBusy(false);
    }
  }

  async function openPreview(document: DocumentAttachment) {
    const version = ++previewVersion.current;
    setPreview({ name: document.name });
    try {
      // Same current default canvas namespace as start_workflow; not a host path.
      const data = await invoke<string>("canvas_document_preview", { reference: document.reference, workspaceId: null });
      const blob = originalBlob(data);
      if (alive.current && previewVersion.current === version) setPreview({ name: document.name, blob });
    } catch {
      if (alive.current && previewVersion.current === version) setPreview({ name: document.name, error: "无法预览原件：文档可能已移除、变更或不属于当前工作区。" });
    }
  }

  return <div className="flex min-w-0 flex-col gap-1.5">
    <input ref={input} id={`${nodeId}-documents`} type="file" className="hidden" multiple accept={ACCEPT}
      aria-label="选择文档附件" disabled={busy} onChange={async event => {
        const files = Array.from(event.currentTarget.files ?? []);
        event.currentTarget.value = "";
        await upload(files);
      }} />
    {entries.map((document, index) => !isAttachment(document)
      ? <div key={`invalid:${index}`} className="flex items-center gap-2">
        <p role="alert" className="flex-1 text-[11px] text-red-300">无效文档引用，请移除后重新选择。</p>
        <Button variant="ghost" size="icon" className={`${CANVAS_BUTTON_CLASS} !h-7 !w-7`} disabled={busy}
          aria-label={`移除无效文档 ${index + 1}`} onClick={() => onChange(entries.filter((_, i) => i !== index))}><X className="h-3 w-3" /></Button>
      </div>
      : <div key={`${document.reference}:${index}`} className="flex min-w-0 items-center gap-1 rounded-md border border-white/10 px-1.5">
      <FileText aria-hidden="true" className="h-3.5 w-3.5 shrink-0 text-white/40" />
      <Button variant="ghost" size="sm" className={`${CANVAS_BUTTON_CLASS} min-w-0 flex-1 justify-start !px-1`}
        aria-label={`预览 ${document.name}`} onClick={() => openPreview(document)}><span className="truncate">{document.name}</span></Button>
      <Button variant="ghost" size="icon" className={`${CANVAS_BUTTON_CLASS} !h-7 !w-7`} disabled={busy}
        aria-label={`移除 ${document.name}`} onClick={() => onChange(entries.filter((_, i) => i !== index))}><X className="h-3 w-3" /></Button>
    </div>)}
    <Button variant="ghost" size="sm" className={`${CANVAS_BUTTON_CLASS} self-start`} disabled={busy || entries.length >= 5}
      onClick={() => input.current?.click()}><Plus className="h-3 w-3" />{busy ? "正在上传…" : `添加文档（${entries.length}/5）`}</Button>
    {error && <p role="alert" className="text-[11px] text-red-300">{error}</p>}
    <Dialog open={preview !== null} onOpenChange={open => { if (!open) { previewVersion.current += 1; setPreview(null); } }}>
      <DialogContent className="flex h-[80vh] w-[min(1000px,90vw)] max-w-none flex-col overflow-hidden p-0" zIndexClass="z-[200]" aria-label="文档原件预览">
        <DialogHeader className="flex flex-row items-center justify-between px-4 py-3">
          <DialogTitle className="truncate text-sm">{preview?.name ?? "文档预览"}</DialogTitle><DialogCloseIcon aria-label="关闭文档预览"><X className="h-4 w-4" /></DialogCloseIcon>
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-hidden bg-white">
          {preview?.blob ? <PreviewBody fileBlob={preview.blob} fileName={preview.name} />
            : <p role={preview?.error ? "alert" : "status"} className="p-4 text-sm text-gray-600">{preview?.error ?? "正在读取原件…"}</p>}
        </div>
      </DialogContent>
    </Dialog>
  </div>;
}
