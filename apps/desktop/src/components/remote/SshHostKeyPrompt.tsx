import { useEffect, useRef, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import type {
  SshAuthenticationChallenge,
  SshHostKeyChallenge,
  SshHostKeyDecision,
} from "../../api";
import { Button } from "../shadcn/button";
import { SshRouteLabel } from "./SshProxyFields";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";
import { createSshConnectionFlow, type SshConfirmationState } from "./SshConnectionFlow";

export function useSshConnectionConfirmation() {
  const [state, setState] = useState<SshConfirmationState>({ challenge: null, authenticationChallenge: null, busy: false });
  const owner = useRef<ReturnType<typeof createSshConnectionFlow> | null>(null);
  if (!owner.current) owner.current = createSshConnectionFlow(setState);
  const flow = owner.current;
  useEffect(() => { flow.activate(); return () => flow.dispose(); }, [flow]);
  return {
    connect: flow.connect,
    prompt: state.challenge
      ? <SshHostKeyPrompt key={state.challenge.challenge_id} challenge={state.challenge} busy={state.busy} onRespond={flow.respond} onCancel={flow.cancel} />
      : state.authenticationChallenge
        ? <SshAuthenticationPrompt key={state.authenticationChallenge.challenge_id} challenge={state.authenticationChallenge} busy={state.busy} onRespond={flow.respondAuthentication} onCancel={flow.cancel} />
        : null,
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
          <DialogDescription><SshRouteLabel proxy={challenge.proxy} jump={challenge.jump} /></DialogDescription>
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

interface AuthenticationProps {
  challenge: SshAuthenticationChallenge;
  busy: boolean;
  onRespond: (responses: string[]) => Promise<void>;
  onCancel: () => Promise<void>;
}

export function SshAuthenticationPrompt({ challenge, busy, onRespond, onCancel }: AuthenticationProps) {
  const { t } = useTranslation();
  const label = (key: string) => t(`settings.connections.authenticationChallenge.${key}`);
  const title = challenge.kind === "private_key" ? t("settings.connections.unlockPrivateKey") : challenge.name || label("title");
  const [responses, setResponses] = useState<string[]>(() => challenge.prompts.map(() => ""));
  const [expired, setExpired] = useState(() => Date.now() >= challenge.expires_at_ms);
  useEffect(() => {
    const remaining = challenge.expires_at_ms - Date.now();
    const expire = () => {
      setResponses(challenge.prompts.map(() => ""));
      setExpired(true);
    };
    if (remaining <= 0) { expire(); return; }
    const timer = window.setTimeout(expire, remaining);
    return () => window.clearTimeout(timer);
  }, [challenge.expires_at_ms, challenge.prompts]);

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || expired) return;
    const submitted = [...responses];
    setResponses(challenge.prompts.map(() => ""));
    await onRespond(submitted);
  };
  const cancel = () => {
    setResponses(challenge.prompts.map(() => ""));
    void onCancel();
  };

  return (
    <Dialog open onOpenChange={(open) => { if (!open && !busy) cancel(); }}>
      <DialogContent className="w-[min(520px,calc(100vw-32px))]" closeOnBackdrop={!busy} closeOnEscape={!busy} aria-label={title}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{challenge.instructions}</DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="space-y-4 px-5 py-4">
          <p className="break-all text-[12px] font-medium">{challenge.username}@{challenge.host}:{challenge.port}</p>
          {challenge.prompts.map((prompt, index) => {
            const id = `ssh-auth-${challenge.challenge_id}-${index}`;
            return (
              <div key={id} className="space-y-1.5">
                <Label htmlFor={id}>{challenge.kind === "private_key" ? label("privateKeyPassphrase") : prompt.prompt}</Label>
                <Input
                  id={id}
                  type={prompt.echo ? "text" : "password"}
                  autoComplete="off"
                  autoFocus={index === 0}
                  value={responses[index] ?? ""}
                  onChange={(event) => setResponses(current => current.map((value, item) => item === index ? event.target.value : value))}
                  disabled={busy || expired}
                />
              </div>
            );
          })}
          {expired && <p role="alert" className="text-red-600">{label("expired")}</p>}
          {busy && <p role="status" aria-live="polite">{label("submitting")}</p>}
          <DialogFooter className="flex-wrap gap-2">
            <Button type="button" variant="outline" disabled={busy} onClick={cancel}>{label("cancel")}</Button>
            <Button type="submit" disabled={busy || expired}>{label("submit")}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
