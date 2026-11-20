import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { fileCommand, type EditorSession } from "../../fileWorkspaceApi";
import { writeClipboardText } from "../../clipboard";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from "../shadcn/dialog";
export function ExternalEditorPanel() {
  const { t } = useTranslation(); const label = (key: string) => t(`remote.files.${key}`);
  const [sessions, setSessions] = useState<EditorSession[]>([]); const [error, setError] = useState("");
  const [decision, setDecision] = useState<{ id: string; action: string } | null>(null); const [path, setPath] = useState(""); const [busy, setBusy] = useState(false);
  useEffect(() => { let live = true; let timer: ReturnType<typeof setTimeout>; const poll = async () => { try { const result = await fileCommand<EditorSession[]>("file_editor_list"); if (live) setSessions(result); } catch (e) { if (live) setError(String(e)); } finally { if (live) timer = setTimeout(() => void poll(), 3000); } }; void poll(); return () => { live = false; clearTimeout(timer); }; }, []);
  async function resolve(id: string, action: string, path?: string) { setBusy(true); setError(""); try { const result = await fileCommand<EditorSession>("file_editor_resolve", { id, action, path: path ?? null }); if (action === "close") setError(`${label("localRetained")}: ${result.localPath}`); setSessions(await fileCommand<EditorSession[]>("file_editor_list")); setDecision(null); } catch (e) { setError(String(e)); } finally { setBusy(false); } }
  return <div className="max-h-48 shrink-0 overflow-auto border-t border-border-theme px-3 text-[12px]">
    {error && <p role="status" className="break-all text-amber-600">{error}</p>}
    {sessions.map(s => <div className="space-y-1 border-b border-border-theme py-2" key={s.id}><p className="break-all">{label("externalEdit")} · {s.remotePath} · {s.state}</p><p className="break-all">{label("recoveryPath")}: {s.localPath}</p>{s.error && <p className="break-all text-amber-600">{s.error}</p>}<div className="flex flex-wrap gap-1">
      <Button size="sm" variant="ghost" onClick={() => void writeClipboardText(s.localPath).catch(e => setError(String(e)))}>{label("copyPath")}</Button>
      <Button size="sm" variant="outline" disabled={busy || !!s.taskId} onClick={() => void resolve(s.id, s.autoUpload ? "pause" : "resume")}>{label(s.autoUpload ? "pause" : "resume")}</Button>
      {["reload", "save_as", "overwrite", "close"].map(action => <Button key={action} size="sm" variant="outline" disabled={busy || !!s.taskId} onClick={() => { setPath(s.remotePath); setDecision({ id: s.id, action }); }}>{label(action)}</Button>)}
    </div></div>)}
    {decision && <Dialog open onOpenChange={open => { if (!open && !busy) setDecision(null); }}><DialogContent><DialogHeader><DialogTitle>{label(decision.action)}</DialogTitle><DialogDescription>{label("editorDecisionDescription")}</DialogDescription></DialogHeader><div className="px-5 py-4">{decision.action === "save_as" ? <Input value={path} onChange={e => setPath(e.target.value)} /> : <p className="break-all">{sessions.find(s => s.id === decision.id)?.remotePath}</p>}</div><DialogFooter><Button variant="outline" onClick={() => setDecision(null)}>{label("close")}</Button><Button disabled={busy} onClick={() => void resolve(decision.id, decision.action, path)}>{label("confirm")}</Button></DialogFooter></DialogContent></Dialog>}
  </div>;
}
