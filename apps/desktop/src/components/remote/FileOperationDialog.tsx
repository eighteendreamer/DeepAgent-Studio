import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Checkbox } from "../shadcn/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from "../shadcn/dialog";
import { fileCommand, previewArchive, previewCopy, probeCopy, submitOperation, type ArchivePreview, type CopyPreview, type CopySpec, type DirectProbe, type WorkbenchOperation } from "../../fileWorkspaceApi";
import { remoteTransferSubmit, type ConflictPolicy } from "../../remoteTransfersApi";
import { joinEndpointPath } from "./filePanelModel";
import type { PanelSelection } from "./FileWorkspace";

export type FileAction = { kind: "copy"; source: PanelSelection; destination: PanelSelection } |
  { kind: "extract"; connectionId: string; path: string; directory: string } |
  { kind: "editor"; connectionId: string; path: string };
type Preview = { operations: WorkbenchOperation[]; copies?: CopyPreview[]; archive?: ArchivePreview; local?: string[] };
export function FileOperationDialog({ action, onClose }: { action: FileAction; onClose: () => void }) {
  const { t } = useTranslation(); const label = (key: string) => t(`remote.files.${key}`);
  const [busy, setBusy] = useState(false); const [error, setError] = useState("");
  const [submitted, setSubmitted] = useState(0);
  const [conflict, setConflict] = useState<ConflictPolicy>("ask");
  const [method, setMethod] = useState<CopySpec["method"]>("relay");
  const [probe, setProbe] = useState<DirectProbe | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [destination, setDestination] = useState(action.kind === "extract" ? action.directory : "");
  const [executable, setExecutable] = useState(() => { try { return localStorage.getItem("remote-editor-executable") ?? ""; } catch { return ""; } });
  const [autoUpload, setAutoUpload] = useState(false);
  async function act(work: () => Promise<void>) { setBusy(true); setError(""); try { await work(); } catch (e) { setError(String(e)); } finally { setBusy(false); } }
  async function plan() {
    setPreview(null);
    if (action.kind === "extract") {
      const archive = await previewArchive({ connectionId: action.connectionId, archivePath: action.path, destinationPath: destination, conflict: conflict as "ask" | "skip" | "replace" });
      setPreview({ archive, operations: [{ operation: "extract", request: { version: archive.version, spec: archive.spec, connectionGeneration: archive.connectionGeneration, fingerprint: archive.fingerprint } }] });
    } else if (action.kind === "copy") {
      const { source, destination } = action;
      if (source.selected.length > 16) throw Error(label("batchLimit"));
      if (source.selected.some(entry => entry.isSymlink)) throw Error(label("linksUnsupported"));
      if (source.endpoint.kind === "local" && destination.endpoint.kind === "local") throw Error(label("chooseRemote"));
      if (source.endpoint.kind === "ssh" && destination.endpoint.kind === "ssh") {
        const copies: CopyPreview[] = [];
        for (const entry of source.selected) copies.push(await previewCopy({ sourceConnectionId: source.endpoint.connectionId, destinationConnectionId: destination.endpoint.connectionId, sourcePath: entry.path, destinationPath: joinEndpointPath(destination.endpoint, destination.cwd, entry.name), conflict, method }));
        setPreview({ copies, operations: copies.map(p => ({ operation: "copy", request: { version: p.version, spec: p.spec, sourceGeneration: p.sourceGeneration, destinationGeneration: p.destinationGeneration, fingerprint: p.fingerprint } })) });
      } else setPreview({ operations: [], local: source.selected.map(entry => `${entry.path} → ${joinEndpointPath(destination.endpoint, destination.cwd, entry.name)}`) });
    }
  }
  async function confirm() {
    if (!preview) return;
    let accepted = submitted;
    for (const operation of preview.operations.slice(submitted)) { await submitOperation(operation); setSubmitted(++accepted); }
    if (action.kind === "copy" && preview.local) {
      const { source, destination } = action;
      for (const entry of source.selected.slice(submitted)) {
        const target = joinEndpointPath(destination.endpoint, destination.cwd, entry.name);
        const upload = source.endpoint.kind === "local";
        const endpoint = upload ? destination.endpoint : source.endpoint;
        if (endpoint.kind !== "ssh") throw Error(label("chooseRemote"));
        await remoteTransferSubmit(endpoint.connectionId, { direction: upload ? "upload" : "download", localPath: upload ? entry.path : target, remotePath: upload ? target : entry.path, conflict }, { kind: entry.isDir ? "directory" : "file", directoryConflict: conflict === "replace" ? "merge" : conflict === "keep_both" ? "keep_both" : conflict });
        setSubmitted(++accepted);
      }
    }
    onClose();
  }
  const canDirect = action.kind === "copy" && action.source.endpoint.kind === "ssh" && action.destination.endpoint.kind === "ssh" && action.source.endpoint.connectionId !== action.destination.endpoint.connectionId;
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose(); }}><DialogContent className="flex max-h-[85vh] w-[min(720px,calc(100vw-32px))] flex-col overflow-hidden">
    <DialogHeader><DialogTitle>{label(action.kind)}</DialogTitle><DialogDescription>{label(action.kind === "editor" ? "editorDescription" : "previewDescription")}</DialogDescription></DialogHeader>
    <div className="space-y-3 overflow-y-auto px-5 py-4 text-[12px]">
      {error && <p role="alert" className="break-all text-red-500">{error}</p>}
      {submitted > 0 && <p role="status">{label("submitted")} {submitted}</p>}
      {action.kind === "editor" ? <>
        <p className="break-all">{action.path}</p><Label>{label("editorExecutable")}</Label><Input value={executable} onChange={e => setExecutable(e.target.value)} />
        <Button variant="outline" onClick={() => void act(async () => { const { open } = await import("@tauri-apps/plugin-dialog"); const path = await open({ multiple: false, directory: false }); if (typeof path === "string") setExecutable(path); })}>{label("chooseEditor")}</Button>
        <Label className="flex gap-2"><Checkbox checked={autoUpload} onCheckedChange={checked => setAutoUpload(checked === true)} />{label("autoUpload")}</Label>
      </> : <>
        {action.kind === "extract" && <><Label>{label("destination")}</Label><Input disabled={busy || submitted > 0} value={destination} onChange={e => { setDestination(e.target.value); setPreview(null); }} /></>}
        {action.kind === "copy" && <div className="break-all"><p>{label("destination")}: {action.destination.cwd}</p>{action.source.selected.slice(0, 16).map(e => <p key={e.path}>{e.path}</p>)}</div>}
        <Label>{label("conflict")}</Label><Select value={conflict} onValueChange={value => { setConflict(value as ConflictPolicy); setPreview(null); }}><SelectTrigger disabled={busy || submitted > 0}><SelectValue /></SelectTrigger><SelectContent>{(action.kind === "extract" ? ["ask", "skip", "replace"] : ["ask", "skip", "replace", "keep_both"]).map(value => <SelectItem key={value} value={value}>{value === "ask" ? label("ask") : t(`remote.transfers.${value === "keep_both" ? "keepBoth" : value}`)}</SelectItem>)}</SelectContent></Select>
        {canDirect && <>
          <Button variant="outline" disabled={busy || submitted > 0} onClick={() => void act(async () => { if (action.kind === "copy" && action.source.endpoint.kind === "ssh" && action.destination.endpoint.kind === "ssh") { setMethod("relay"); setProbe(await probeCopy(action.source.endpoint.connectionId, action.destination.endpoint.connectionId)); setPreview(null); } })}>{label("probeDirect")}</Button>
          {probe && <p className="break-all">{probe.destinationFingerprint} · {probe.authenticated ? label("directReady") : probe.reason}</p>}
          <Select value={method} onValueChange={value => { setMethod(value as CopySpec["method"]); setPreview(null); }}><SelectTrigger disabled={busy || submitted > 0}><SelectValue /></SelectTrigger><SelectContent><SelectItem value="relay">{label("relay")}</SelectItem><SelectItem value="scp" disabled={!probe?.authenticated || !probe.scp}>SCP / SFTP</SelectItem><SelectItem value="rsync" disabled={!probe?.authenticated || !probe.rsync}>rsync</SelectItem></SelectContent></Select>
          <p>{label("directDescription")}</p>
        </>}
        {preview && <div role="status" className="space-y-1 rounded border border-border-theme p-2">
          {preview.copies?.map(p => <p key={p.spec.sourcePath} className="break-all">{p.spec.sourcePath} · {p.totalFiles} {label("filesCount")} · {p.totalBytes} B · {p.conflicts} {label("conflicts")} · {p.spec.method}</p>)}
          {preview.archive && <><p>{preview.archive.details.format} · {preview.archive.details.totalBytes} B · {preview.archive.details.conflicts} {label("conflicts")}</p><p>{label("archiveQuota")}: {preview.archive.details.maxEntries} / {preview.archive.details.maxBytes} B</p><div className="max-h-52 overflow-auto">{preview.archive.details.entries.map(e => <p key={e.path} className="break-all">{e.path} · {e.sizeBytes} B</p>)}</div></>}
          {preview.local?.map(path => <p className="break-all" key={path}>{path}</p>)}
          <p>{label("partialWarning")}</p>
        </div>}
      </>}
    </div><DialogFooter>
      <Button variant="outline" disabled={busy} onClick={onClose}>{label("close")}</Button>
      {action.kind === "editor" ? <Button disabled={busy || !executable} onClick={() => void act(async () => { await fileCommand("file_editor_open", { request: { connectionId: action.connectionId, remotePath: action.path, executable, autoUpload } }); try { localStorage.setItem("remote-editor-executable", executable); } catch { /* Editing remains available when preferences are disabled. */ } onClose(); })}>{label("launchEditor")}</Button> : <>
        <Button variant="outline" disabled={busy || submitted > 0} onClick={() => void act(plan)}>{label("preview")}</Button>
        <Button disabled={busy || !preview || conflict === "ask" && ((preview.copies?.some(p => p.conflicts > 0) ?? false) || (preview.archive?.details.conflicts ?? 0) > 0)} onClick={() => void act(confirm)}>{label("confirm")}</Button>
      </>}
    </DialogFooter>
  </DialogContent></Dialog>;
}
