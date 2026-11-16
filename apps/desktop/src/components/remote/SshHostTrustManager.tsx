import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  sshImportHostTrust, sshInspectHostTrust, sshListHostKeys, sshRevokeHostKey,
  type SshConnection, type SshHostKeyRecord, type SshHostTrustInspection,
} from "../../api";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Label } from "../shadcn/label";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "../shadcn/dialog";

interface Props {
  connection: SshConnection | null;
  onClose: () => void;
  onChanged: () => void;
}

export function SshHostTrustManager({ connection, onClose, onChanged }: Props) {
  if (!connection) return null;
  // A new endpoint/source gets a new owner; never show another host's first frame.
  const scope = JSON.stringify([connection.id, connection.host, connection.port, connection.known_hosts_path]);
  return <HostTrustPanel key={scope} connection={connection} onClose={onClose} onChanged={onChanged} />;
}

function HostTrustPanel({ connection, onClose, onChanged }: Props & { connection: SshConnection }) {
  const { t } = useTranslation();
  const [records, setRecords] = useState<SshHostKeyRecord[]>([]);
  const [preview, setPreview] = useState<SshHostTrustInspection | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [revoking, setRevoking] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const pending = useRef(false);
  const label = (key: string) => t(`settings.connections.trustManager.${key}`);

  async function run(action: "load" | "preview" | "import" | "revoke") {
    if (pending.current || (action === "import" && (!preview || !acknowledged)) || (action === "revoke" && !revoking)) return;
    pending.current = true;
    const request = ++generation.current;
    setBusy(true);
    setError("");
    try {
      if (action === "preview") {
        const inspected = await sshInspectHostTrust(connection.host, connection.port, connection.known_hosts_path);
        if (request !== generation.current) return;
        setPreview(inspected);
        setAcknowledged(false);
        setRevoking(null);
      } else {
        let next: SshHostKeyRecord[];
        if (action === "import") {
          next = await sshImportHostTrust(connection.id, preview!);
        } else if (action === "revoke") {
          await sshRevokeHostKey(connection.id, revoking!);
          next = await sshListHostKeys(connection.id);
        } else {
          next = await sshListHostKeys(connection.id);
        }
        if (request !== generation.current) return;
        setRecords(next);
        setPreview(null);
        setRevoking(null);
        setAcknowledged(false);
        if (action !== "load") onChanged();
      }
    } catch (failure) {
      if (request === generation.current) setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      if (request === generation.current) {
        pending.current = false;
        setBusy(false);
      }
    }
  }

  useEffect(() => {
    void run("load");
    return () => { generation.current++; pending.current = false; };
  }, []);

  const acknowledgementId = `ssh-trust-ack-${connection.id}`;
  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="flex max-h-[85vh] w-[min(620px,calc(100vw-32px))] flex-col overflow-hidden" aria-label={label("title")}>
        <DialogHeader>
          <DialogTitle>{label("title")}</DialogTitle>
          <DialogDescription className="break-all">{connection.host}:{connection.port} · {label("direct")}</DialogDescription>
        </DialogHeader>
        <div className="min-h-0 space-y-4 overflow-y-auto px-5 py-4 text-[12px]">
          <p className="text-text-secondary">{label("help")}</p>
          {busy && <p role="status" aria-live="polite">{label("busy")}</p>}
          {error && <p role="alert" className="break-all rounded-lg bg-red-50 p-3 text-red-600">{error}</p>}
          {!busy && records.length === 0 && <p className="text-text-secondary">{label("empty")}</p>}
          {records.map((record) => (
            <section key={record.fingerprint} className="space-y-1.5 rounded-xl border border-border-theme p-3">
              <div className="flex items-center justify-between gap-3">
                <span>{record.algorithm} · {label(record.revoked ? "revoked" : "trusted")}</span>
                <Button size="sm" variant="ghost" disabled={busy || record.revoked} onClick={() => { setRevoking(record.fingerprint); setPreview(null); setError(""); }}>{label("revoke")}</Button>
              </div>
              <p className="break-all font-mono">{record.fingerprint}</p>
              <p className="break-all text-text-secondary">{record.source}</p>
              <p className="text-text-secondary">{new Date(record.trusted_at_ms).toLocaleString()}</p>
            </section>
          ))}
          {preview && (
            <section className="space-y-3 rounded-xl border border-border-theme p-3">
              <h3 className="font-medium">{label("review")}</h3>
              <p className="break-all text-text-secondary">{preview.host}:{preview.port} · {preview.known_hosts_path}</p>
              {preview.keys.map((key) => <p key={`${key.line_number}:${key.fingerprint}`} className="break-all font-mono">{key.algorithm} · {key.fingerprint}</p>)}
              <div className="flex items-start gap-2">
                <Checkbox id={acknowledgementId} checked={acknowledged} disabled={busy} onCheckedChange={(value) => setAcknowledged(value === true)} />
                <Label htmlFor={acknowledgementId} className="text-[12px] leading-5">{label("acknowledge")}</Label>
              </div>
              <div className="flex flex-wrap gap-2">
                <Button size="sm" disabled={busy || !acknowledged} onClick={() => run("import")}>{label("confirmImport")}</Button>
                <Button size="sm" variant="outline" disabled={busy} onClick={() => { setPreview(null); setAcknowledged(false); }}>{label("cancel")}</Button>
              </div>
            </section>
          )}
          {revoking && (
            <section className="space-y-3 rounded-xl border border-red-200 bg-red-50 p-3">
              <p>{label("revokeWarning")}</p><p className="break-all font-mono">{revoking}</p>
              <div className="flex gap-2">
                <Button size="sm" variant="destructive" disabled={busy} onClick={() => run("revoke")}>{label("confirmRevoke")}</Button>
                <Button size="sm" variant="outline" disabled={busy} onClick={() => setRevoking(null)}>{label("cancel")}</Button>
              </div>
            </section>
          )}
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" disabled={busy} onClick={() => run("preview")}>{label("preview")}</Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => run("load")}>{label("refresh")}</Button>
          </div>
          <p className="text-text-secondary">{label("sessionWarning")}</p>
        </div>
        <DialogFooter><Button variant="outline" onClick={onClose}>{label("close")}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
