import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  sshBeginHostKeyRotation, sshConfirmHostKeyRotation, sshCancelHostKey,
  type SshConnection, type SshHostKeyRotationPreview, type SshHostKeyRotationResult,
} from "../../api";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Label } from "../shadcn/label";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";

interface Props {
  connection: SshConnection;
  onBack: () => void;
  onCommitted: () => void;
}

/** Separate management surface, not an "ignore changed key" connection prompt. */
export function SshHostKeyRotationDialog({ connection, onBack, onCommitted }: Props) {
  const { t } = useTranslation();
  const label = (key: string) => t(`settings.connections.hostRotation.${key}`);
  const [preview, setPreview] = useState<SshHostKeyRotationPreview | null>(null);
  const [receipt, setReceipt] = useState<SshHostKeyRotationResult | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [expired, setExpired] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const generation = useRef(0);
  const current = useRef<SshHostKeyRotationPreview | null>(null);

  async function cancel(value: SshHostKeyRotationPreview) {
    const c = value.challenge;
    await sshCancelHostKey(c.connection_id, c.attempt_id, c.challenge_id);
  }

  useEffect(() => () => {
    generation.current++; pending.current = false;
    const value = current.current; current.current = null;
    if (value) void cancel(value).catch((failure) => console.warn("Abandoned SSH rotation cancellation failed; backend TTL still applies", failure));
  }, []);

  useEffect(() => {
    if (!preview) return;
    const remaining = preview.challenge.expires_at_ms - Date.now();
    setExpired(remaining <= 0);
    if (remaining <= 0) return;
    const timer = window.setTimeout(() => setExpired(true), remaining);
    return () => window.clearTimeout(timer);
  }, [preview]);

  async function run(action: "probe" | "confirm" | "back") {
    if (pending.current || (action === "confirm" && (!preview || !acknowledged || Date.now() >= preview.challenge.expires_at_ms))) return;
    pending.current = true;
    const owner = ++generation.current;
    setBusy(true); setError("");
    try {
      if (action === "confirm") {
        const c = preview!.challenge;
        current.current = null; // Response is consumed even if its validation fails.
        const result = await sshConfirmHostKeyRotation(c.connection_id, c.attempt_id, c.challenge_id);
        if (owner !== generation.current) return;
        setReceipt(result); setPreview(null); setAcknowledged(false); onCommitted();
      } else {
        if (current.current) { await cancel(current.current); current.current = null; }
        if (owner !== generation.current) return;
        setPreview(null); setAcknowledged(false); setExpired(false);
        if (action === "back") { onBack(); return; }
        setReceipt(null);
        const result = await sshBeginHostKeyRotation(connection.id);
        if (owner !== generation.current) { await cancel(result); return; }
        const c = result.challenge;
        if (c.connection_id !== connection.id || c.host !== connection.host || c.port !== connection.port || c.kind !== "key_rotation" || c.route !== "direct" || c.role !== "target") {
          await cancel(result);
          throw new Error(label("unexpected"));
        }
        current.current = result; setPreview(result);
      }
    } catch (failure) {
      if (owner === generation.current) {
        setError(failure instanceof Error ? failure.message : String(failure));
        if (action === "confirm") { setPreview(null); setAcknowledged(false); }
      }
    } finally {
      if (owner === generation.current) { pending.current = false; setBusy(false); }
    }
  }

  const isExpired = expired || (preview !== null && Date.now() >= preview.challenge.expires_at_ms);
  const ackId = `ssh-rotation-ack-${connection.id}`;
  return <Dialog open onOpenChange={(open) => { if (!open && !busy) void run("back"); }}>
    <DialogContent className="flex max-h-[85vh] w-[min(640px,calc(100vw-32px))] flex-col overflow-hidden" closeOnBackdrop={!busy} closeOnEscape={!busy} aria-label={label("title")}>
      <DialogHeader><DialogTitle>{label("title")}</DialogTitle><DialogDescription>{connection.host}:{connection.port} · {label("direct")}</DialogDescription></DialogHeader>
      <div className="min-h-0 space-y-4 overflow-y-auto px-5 py-4 text-[12px]">
        <p className="leading-5 text-text-secondary">{label("warning")}</p>
        {busy && <p role="status" aria-live="polite">{label("busy")}</p>}
        {error && <p role="alert" className="break-all text-red-600">{error}</p>}
        {preview && <>
          <section className="space-y-2 rounded-xl border border-border-theme p-3">
            <h3 className="font-medium">{label("previous")}</h3>
            {preview.previous_keys.length === 0 && <p>{label("alreadyRevoked")}</p>}
            {preview.previous_keys.map((key) => <p key={key.fingerprint} className="break-all font-mono">{key.algorithm} · {key.fingerprint}</p>)}
          </section>
          <section className="space-y-2 rounded-xl border border-border-theme p-3">
            <h3 className="font-medium">{label("replacement")}</h3>
            <p>{preview.challenge.algorithm}</p><p className="break-all font-mono">{preview.challenge.fingerprint}</p>
            <p>{label("expires")} {new Date(preview.challenge.expires_at_ms).toLocaleString()}</p>
          </section>
          {isExpired && <p role="alert" className="text-red-600">{label("expired")}</p>}
          <div className="flex items-start gap-2"><Checkbox id={ackId} checked={acknowledged} disabled={busy || isExpired} onCheckedChange={(value) => setAcknowledged(value === true)} /><Label htmlFor={ackId} className="text-[12px] leading-5">{label("acknowledge")}</Label></div>
        </>}
        {receipt && <section className="space-y-2 rounded-xl border border-border-theme p-3" role="status">
          <p>{label("committed")}</p><p className="break-all font-mono">{receipt.audit.algorithm} · {receipt.audit.fingerprint}</p>
          <p>{label("revision")} {receipt.audit.revision} · {new Date(receipt.audit.at_ms).toLocaleString()}</p>
          {receipt.disconnect_failed_ids.length > 0 && <p role="alert">{label("cleanupFailed")} {receipt.disconnect_failed_ids.join(", ")}</p>}
        </section>}
      </div>
      <DialogFooter className="flex-wrap gap-2">
        <Button variant="outline" disabled={busy} onClick={() => run("back")}>{label("back")}</Button>
        <Button variant="outline" disabled={busy} onClick={() => run("probe")}>{label("probe")}</Button>
        {preview && <Button variant="destructive" disabled={busy || isExpired || !acknowledged} onClick={() => run("confirm")}>{label("confirm")}</Button>}
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
