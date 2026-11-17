import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshHostTrustAudit, type SshConnection, type SshHostTrustAudit } from "../../api";
import { Button } from "../shadcn/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";

/** Read-only bounded snapshots. No polling, SSH probe or mutation is performed. */
export function SshHostTrustAuditDialog({ connection, onBack }: {
  connection: SshConnection; onBack: () => void;
}) {
  const { t } = useTranslation();
  const label = (key: string) => t(`settings.connections.trustAudit.${key}`);
  const [snapshot, setSnapshot] = useState<SshHostTrustAudit | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [page, setPage] = useState(0);
  const generation = useRef(0);
  const pending = useRef(false);

  async function load() {
    if (pending.current) return;
    pending.current = true;
    const owner = ++generation.current;
    setBusy(true); setError(""); setSnapshot(null);
    try {
      const result = await sshHostTrustAudit(connection.id);
      if (owner !== generation.current) return;
      if (result.connection_id !== connection.id || result.host !== connection.host || result.port !== connection.port || result.route !== "direct" || !Array.isArray(result.records) || result.records.length > 256) {
        throw new Error(label("unexpected"));
      }
      setSnapshot(result); setPage(0);
    } catch (failure) {
      if (owner === generation.current) setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      if (owner === generation.current) { pending.current = false; setBusy(false); }
    }
  }

  useEffect(() => {
    void load();
    return () => { generation.current++; pending.current = false; };
  }, []);

  const records = snapshot?.records.slice().reverse() ?? [];
  const pageSize = 10;
  const pages = Math.max(1, Math.ceil(records.length / pageSize));
  const current = Math.min(page, pages - 1);
  const visible = records.slice(current * pageSize, (current + 1) * pageSize);
  return <Dialog open onOpenChange={(open) => { if (!open) onBack(); }}>
    <DialogContent className="flex max-h-[85vh] w-[min(660px,calc(100vw-32px))] flex-col overflow-hidden" aria-label={label("title")}>
      <DialogHeader><DialogTitle>{label("title")}</DialogTitle><DialogDescription>{connection.host}:{connection.port} · {label("direct")}</DialogDescription></DialogHeader>
      <div className="min-h-0 space-y-3 overflow-y-auto px-5 py-4 text-[12px]">
        <p className="leading-5 text-text-secondary">{label("help")}</p>
        {busy && <p role="status" aria-live="polite">{label("busy")}</p>}
        {error && <p role="alert" className="break-all text-red-600">{error}</p>}
        {snapshot && <p>{label("revision")} {snapshot.revision} · {label("count")} {records.length}</p>}
        {snapshot && <section className="space-y-1 rounded-xl border border-border-theme p-3">
          <p>{label("lastUsed")} {snapshot.last_used_at_ms == null ? label("usageUnknown") : new Date(snapshot.last_used_at_ms).toLocaleString()}</p>
          <p className="text-text-secondary">{label("usageHelp")}</p>
          {snapshot.activity_error && <p role="alert" className="break-all text-red-600">{label("usageError")} {snapshot.activity_error}</p>}
        </section>}
        {snapshot && records.length === 0 && <p>{label("empty")}</p>}
        {visible.map((record) => <article key={record.revision} data-audit-revision={record.revision} className="space-y-2 rounded-xl border border-border-theme p-3">
          <p className="font-medium">#{record.revision} · {label(`action.${record.action}`)}</p>
          <p className="text-text-secondary">{new Date(record.at_ms).toLocaleString()}</p>
          {record.trusted_fingerprints.length > 0 && <section><p>{label("trusted")}</p>{record.trusted_fingerprints.map((key) => <p key={key} className="break-all font-mono">{key}</p>)}</section>}
          {record.revoked_fingerprints.length > 0 && <section><p>{label("revoked")}</p>{record.revoked_fingerprints.map((key) => <p key={key} className="break-all font-mono">{key}</p>)}</section>}
          {record.replacement && <section><p>{label("replacement")}</p><p className="break-all font-mono">{record.replacement.algorithm} · {record.replacement.fingerprint}</p></section>}
        </article>)}
      </div>
      <DialogFooter className="flex-wrap items-center gap-2">
        <Button variant="outline" onClick={onBack}>{label("back")}</Button>
        <Button variant="outline" disabled={busy} onClick={load}>{label("refresh")}</Button>
        {pages > 1 && <><Button variant="outline" disabled={current === 0 || busy} onClick={() => setPage(current - 1)}>{label("previous")}</Button><span>{current + 1} / {pages}</span><Button variant="outline" disabled={current + 1 === pages || busy} onClick={() => setPage(current + 1)}>{label("next")}</Button></>}
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
