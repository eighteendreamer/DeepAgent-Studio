import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { SshHostKeyChallenge, SshHostKeyDecision } from "../../api";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Label } from "../shadcn/label";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";
import { createSshConnectionFlow, type SshConfirmationState } from "./SshConnectionFlow";

export function useSshConnectionConfirmation() {
  const [state, setState] = useState<SshConfirmationState>({ challenge: null, busy: false });
  const owner = useRef<ReturnType<typeof createSshConnectionFlow> | null>(null);
  if (!owner.current) owner.current = createSshConnectionFlow(setState);
  const flow = owner.current;
  useEffect(() => { flow.activate(); return () => flow.dispose(); }, [flow]);
  return {
    connect: flow.connect,
    prompt: state.challenge ? <SshHostKeyPrompt key={state.challenge.challenge_id} challenge={state.challenge} busy={state.busy} onRespond={flow.respond} onCancel={flow.cancel} /> : null,
  };
}

interface Props {
  challenge: SshHostKeyChallenge;
  busy: boolean;
  onRespond: (decision: SshHostKeyDecision) => Promise<void>;
  onCancel: () => Promise<void>;
}

function SshHostKeyPrompt({ challenge, busy, onRespond, onCancel }: Props) {
  const { t } = useTranslation();
  const label = (key: string) => t(`settings.connections.hostChallenge.${key}`);
  const [acknowledged, setAcknowledged] = useState(false);
  const [expired, setExpired] = useState(() => Date.now() >= challenge.expires_at_ms);
  useEffect(() => {
    const remaining = challenge.expires_at_ms - Date.now();
    if (remaining <= 0) { setExpired(true); return; }
    const timer = window.setTimeout(() => setExpired(true), remaining);
    return () => window.clearTimeout(timer);
  }, [challenge.expires_at_ms]);
  const disabled = busy || expired || !acknowledged;
  const acknowledgementId = `ssh-host-challenge-${challenge.challenge_id}`;
  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) void onCancel(); }}>
      <DialogContent className="w-[min(600px,calc(100vw-32px))]" closeOnBackdrop={!busy} closeOnEscape={!busy} aria-label={label("title")}>
        <DialogHeader>
          <DialogTitle>{label("title")}</DialogTitle>
          <DialogDescription>{label("targetDirect")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-4 px-5 py-4 text-[12px]">
          <p className="break-all font-medium">{challenge.username}@{challenge.host}:{challenge.port}</p>
          <p className="leading-5 text-text-secondary">{label("warning")}</p>
          <section className="space-y-2 rounded-xl border border-border-theme bg-black/[0.02] p-3">
            <p>{challenge.algorithm}</p><p className="break-all font-mono">{challenge.fingerprint}</p>
            <p className="text-text-secondary">{label("expires")} {new Date(challenge.expires_at_ms).toLocaleString()}</p>
          </section>
          {expired && <p role="alert" className="text-red-600">{label("expired")}</p>}
          {busy && <p role="status" aria-live="polite">{label("connecting")}</p>}
          <div className="flex items-start gap-2">
            <Checkbox id={acknowledgementId} checked={acknowledged} disabled={busy || expired} onCheckedChange={(value) => setAcknowledged(value === true)} />
            <Label htmlFor={acknowledgementId} className="text-[12px] leading-5">{label("acknowledge")}</Label>
          </div>
          <p className="leading-5 text-text-secondary">{label("scope")}</p>
        </div>
        <DialogFooter className="flex-wrap gap-2">
          <Button variant="outline" disabled={busy} onClick={() => void onCancel()}>{label("cancel")}</Button>
          <Button variant="outline" disabled={disabled} onClick={() => void onRespond("once")}>{label("once")}</Button>
          {challenge.can_save && <Button disabled={disabled} onClick={() => void onRespond("save")}>{label("save")}</Button>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
