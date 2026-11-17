import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter,
} from "../shadcn/dialog";
import {
  remoteTransferSubmit, remoteTransferCancel,
  remoteTransferRetry, remoteTransfersClear, type TransferTask, type TransferDirection,
} from "../../remoteTransfersApi";
import { observeRemoteTransfers } from "../../remoteTransferObserver";

type Labeler = (key: string) => string;

export function TransferTaskBody({ task, label }: { task: TransferTask; label: Labeler }) {
  const warnings = [
    ...(task.error?.cleanupWarnings ?? []), ...(task.receipt?.cleanupWarnings ?? []),
  ];
  return <div className="min-w-0 flex-1 space-y-1 text-[12px]">
    <p className="break-all">{label(task.request.transfer.direction)} · {task.request.transfer.remotePath}</p>
    <p className="break-all text-text-secondary">{task.request.transfer.localPath}</p>
    <p>{label(task.state)} · {task.bytes} / {task.total ?? "?"} B · {label("attempt")} {task.attempt}</p>
    {task.error && <p role="alert" className="break-words text-red-500">{task.error.code}: {task.error.message}</p>}
    {task.receipt && <p className="break-all text-text-secondary">
      {task.receipt.skipped ? label("skipped") : task.receipt.publishStrategy} {task.receipt.sha256}
    </p>}
    {warnings.map(warning => <p key={warning} className="break-all text-amber-600">{warning}</p>)}
  </div>;
}

/** Backend snapshots own task state; unmounting only stops observation, never work. */
export function RemoteTransfersPanel({ connectionId }: { connectionId: string }) {
  const { t } = useTranslation();
  const label: Labeler = key => t(`remote.transfers.${key}`);
  const [allTasks, setTasks] = useState<TransferTask[]>([]);
  const tasks = allTasks.filter(task => task.request.connectionId === connectionId);
  const [error, setError] = useState("");
  const [expanded, setExpanded] = useState(false);
  const [form, setForm] = useState<TransferDirection | null>(null);
  const [local, setLocal] = useState("");
  const [remote, setRemote] = useState("");
  const [busy, setBusy] = useState(false);
  const [replace, setReplace] = useState<string | null>(null);
  const replacement = tasks.find(task => task.id === replace)?.request.transfer;

  useEffect(() => observeRemoteTransfers({
    next: snapshot => { setTasks(snapshot.tasks); setError(""); },
    error: error => setError(String(error)),
  }), []);

  async function act(work: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await work();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  function openForm(direction: TransferDirection) {
    setForm(direction);
    setLocal("");
    setRemote("");
    setError("");
  }
  async function choose() {
    try {
      const dialog = await import("@tauri-apps/plugin-dialog");
      const path = form === "download"
        ? await dialog.save({ defaultPath: remote.split("/").pop() || "download" })
        : await dialog.open({ multiple: false, directory: false });
      if (typeof path === "string") {
        setLocal(path);
        if (!remote) setRemote("/" + path.split(/[\\/]/).pop());
      }
    } catch (e) {
      setError(String(e));
    }
  }

  return <section className="shrink-0 border-t border-border-theme bg-elevated-bg px-3 py-2" aria-label={label("title")}>
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="ghost" size="sm" onClick={() => setExpanded(value => !value)}>{label("title")} ({tasks.length})</Button>
      <Button variant="outline" size="sm" onClick={() => openForm("upload")}>{label("upload")}</Button>
      <Button variant="outline" size="sm" onClick={() => openForm("download")}>{label("download")}</Button>
      <Button variant="ghost" size="sm" disabled={busy} onClick={() => void act(() => remoteTransfersClear())}>{label("clear")}</Button>
    </div>
    {error && <p role="alert" className="break-all text-[12px] text-red-500">{error}</p>}
    {expanded && <div className="max-h-[220px] space-y-2 overflow-y-auto pt-2">
      {tasks.length === 0 && <p className="text-[12px] text-text-secondary">{label("empty")}</p>}
      {tasks.slice().reverse().map(task => <article key={task.id} className="flex items-start gap-2 rounded-lg border border-border-theme p-2">
        <TransferTaskBody task={task} label={label} />
        <div className="flex shrink-0 flex-col gap-1">
          {["queued", "running"].includes(task.state) && <Button variant="outline" size="sm" disabled={busy}
            onClick={() => void act(() => remoteTransferCancel(task.id))}>{label("cancel")}</Button>}
          {["failed", "cancelled"].includes(task.state) && <Button variant="outline" size="sm" disabled={busy}
            onClick={() => void act(() => remoteTransferRetry(task.id))}>{label("retry")}</Button>}
          {task.error?.code === "conflict" && <>
            <Button variant="outline" size="sm" disabled={busy}
              onClick={() => void act(() => remoteTransferRetry(task.id, "skip"))}>{label("skip")}</Button>
            <Button variant="destructive" size="sm" disabled={busy} onClick={() => setReplace(task.id)}>{label("replace")}</Button>
          </>}
        </div>
      </article>)}
    </div>}
    {form && <Dialog open onOpenChange={open => { if (!open && !busy) setForm(null); }}>
      <DialogContent className="flex max-h-[85vh] w-[min(640px,calc(100vw-32px))] flex-col overflow-hidden" aria-label={label(form)}>
        <DialogHeader><DialogTitle>{label(form)}</DialogTitle><DialogDescription>{label("safe")}</DialogDescription></DialogHeader>
        <div className="space-y-3 overflow-y-auto px-5 py-4">
          <Label htmlFor="transfer-remote">{label("remotePath")}</Label>
          <Input id="transfer-remote" value={remote} onChange={e => setRemote(e.target.value)} placeholder="/home/user/file.bin" />
          <Label htmlFor="transfer-local">{label("localPath")}</Label>
          <div className="flex gap-2">
            <Input id="transfer-local" value={local} onChange={e => setLocal(e.target.value)} />
            <Button variant="outline" onClick={() => void choose()}>{label("choose")}</Button>
          </div>
          {error && <p role="alert" className="text-red-500">{error}</p>}
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={busy} onClick={() => setForm(null)}>{label("close")}</Button>
          <Button disabled={busy || !local || !remote.startsWith("/")} onClick={() => void act(async () => {
            await remoteTransferSubmit(connectionId, { direction: form, localPath: local, remotePath: remote });
            setForm(null);
            setExpanded(true);
          })}>{label("enqueue")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>}
    {replace && <Dialog open onOpenChange={open => { if (!open && !busy) setReplace(null); }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{label("replace")}</DialogTitle><DialogDescription>{label("replaceWarning")}</DialogDescription></DialogHeader>
        <div className="break-all px-5 py-4 text-[12px]">
          {replacement && <>{label(replacement.direction)} · {replacement.localPath} ↔ {replacement.remotePath}</>}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => setReplace(null)}>{label("close")}</Button>
          <Button variant="destructive" disabled={busy || !replacement} onClick={() => void act(async () => {
            await remoteTransferRetry(replace, "replace");
            setReplace(null);
          })}>{label("confirmReplace")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>}
  </section>;
}
