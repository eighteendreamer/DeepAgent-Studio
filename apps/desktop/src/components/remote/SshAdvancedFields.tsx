import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshAddAgentIdentity, sshPreviewAgentIdentity, sshProbeX11, type SshAdvancedOptions, type SshAgentIdentity, type SshAgentProvider } from "../../api";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";

export const defaultSshAdvanced: SshAdvancedOptions = { forward_agent: false, agent_provider: "auto", allow_legacy_algorithms: false, x11: null };

export function SshAdvancedFields({ value, onChange }: { value?: SshAdvancedOptions; onChange: (value: SshAdvancedOptions) => void }) {
  const { t } = useTranslation();
  const id = useId();
  const current = value ?? defaultSshAdvanced;
  const probeRevision = useRef(0);
  const [probe, setProbe] = useState<string>("");
  const [probing, setProbing] = useState(false);
  const update = (change: Partial<SshAdvancedOptions>) => { ++probeRevision.current; setProbe(""); onChange({ ...current, ...change }); };
  const label = (key: string) => t(`settings.connections.advanced.${key}`);
  useEffect(() => { ++probeRevision.current; setProbe(""); }, [current.x11?.display, current.x11?.xauthority_path]);
  useEffect(() => () => { ++probeRevision.current; }, []);
  return <div className="space-y-3" data-ssh-advanced="true">
    <Label>{label("title")}</Label>
    <div className="flex items-center gap-2"><Checkbox id={`${id}-legacy`} checked={current.allow_legacy_algorithms}
      onCheckedChange={checked => update({ allow_legacy_algorithms: checked === true })} />
      <Label htmlFor={`${id}-legacy`}>{label("legacy")}</Label></div>
    {current.allow_legacy_algorithms && <p className="text-xs text-text-secondary">{label("legacyHint")}</p>}
    <div className="flex items-center gap-2"><Checkbox id={`${id}-agent`} checked={current.forward_agent}
      onCheckedChange={checked => update({ forward_agent: checked === true })} />
      <Label htmlFor={`${id}-agent`}>{label("forwardAgent")}</Label></div>
    {current.forward_agent && <p className="text-xs text-text-secondary">{label("forwardAgentHint")}</p>}
    <Label htmlFor={`${id}-provider`}>{label("provider")}</Label>
    <Select value={current.agent_provider} onValueChange={provider => update({ agent_provider: provider as SshAgentProvider })}>
      <SelectTrigger id={`${id}-provider`}><SelectValue /></SelectTrigger><SelectContent>
        <SelectItem value="auto">Auto</SelectItem><SelectItem value="open_ssh">OpenSSH</SelectItem><SelectItem value="pageant">Pageant</SelectItem>
      </SelectContent></Select>
    <div className="flex items-center gap-2"><Checkbox id={`${id}-x11`} checked={current.x11 != null}
      onCheckedChange={checked => update({ x11: checked ? { display: "127.0.0.1:0", xauthority_path: null } : null })} />
      <Label htmlFor={`${id}-x11`}>{label("x11")}</Label></div>
    {current.x11 && <div className="grid grid-cols-2 gap-3">
      <div className="space-y-1.5"><Label htmlFor={`${id}-display`}>{label("display")}</Label><Input id={`${id}-display`} value={current.x11.display}
        onChange={event => update({ x11: { ...current.x11!, display: event.target.value } })} /></div>
      <div className="space-y-1.5"><Label htmlFor={`${id}-authority`}>{label("authority")}</Label><Input id={`${id}-authority`} value={current.x11.xauthority_path ?? ""}
        onChange={event => update({ x11: { ...current.x11!, xauthority_path: event.target.value || null } })} /></div>
      <Button type="button" variant="outline" disabled={probing} onClick={async () => {
        setProbing(true); setProbe(""); const options = current.x11!; const round = ++probeRevision.current;
        try { const result = await sshProbeX11(options); if (round === probeRevision.current) setProbe(result.available ? label("available") : result.reason ?? label("unavailable")); }
        catch (error) { if (round === probeRevision.current) setProbe(String(error)); } finally { setProbing(false); }
      }}>{label("probe")}</Button>
      <div className="text-xs break-all" role="status">{probe}</div>
    </div>}
    <AgentIdentityManager key={current.agent_provider} provider={current.agent_provider} />
  </div>;
}

function AgentIdentityManager({ provider }: { provider: SshAgentProvider }) {
  const { t } = useTranslation(); const id = useId();
  const label = (key: string) => t(`settings.connections.advanced.${key}`);
  const [path, setPath] = useState(""); const [passphrase, setPassphrase] = useState("");
  const [lifetime, setLifetime] = useState("3600"); const [identity, setIdentity] = useState<SshAgentIdentity | null>(null);
  const [authorized, setAuthorized] = useState(false); const [busy, setBusy] = useState(false); const [status, setStatus] = useState("");
  const reset = () => { setIdentity(null); setAuthorized(false); setStatus(""); };
  useEffect(() => { reset(); setPassphrase(""); }, [provider]);
  const request = () => ({ provider, key_path: path, key_content: null, passphrase: passphrase || null,
    lifetime_secs: Number(lifetime), expected_fingerprint: identity?.fingerprint ?? "", authorized });
  const validLifetime = /^\d+$/.test(lifetime) && Number(lifetime) >= 1 && Number(lifetime) <= 86400;
  return <div className="space-y-2 border-t border-border-theme pt-3">
    <div className="text-sm font-medium">{label("addTitle")}</div>
    <Label className="block" htmlFor={`${id}-key`}>{label("key")}</Label><Input id={`${id}-key`} disabled={busy} value={path} onChange={event => { reset(); setPath(event.target.value); setPassphrase(""); }} />
    <div className="grid grid-cols-2 gap-3"><div className="space-y-1.5"><Label htmlFor={`${id}-passphrase`}>{label("passphrase")}</Label>
      <Input id={`${id}-passphrase`} disabled={busy} type="password" autoComplete="new-password" value={passphrase} onChange={event => { reset(); setPassphrase(event.target.value); }} /></div>
      <div className="space-y-1.5"><Label htmlFor={`${id}-lifetime`}>{label("lifetime")}</Label><Input id={`${id}-lifetime`} disabled={busy} type="number" min={1} max={86400} value={lifetime}
        onChange={event => { setAuthorized(false); setLifetime(event.target.value); }} /></div></div>
    <Button type="button" variant="outline" disabled={busy || !path.trim() || !validLifetime} onClick={async () => {
      setBusy(true); setStatus(""); setAuthorized(false);
      try { setIdentity(await sshPreviewAgentIdentity(request())); } catch (error) { setIdentity(null); setStatus(String(error)); } finally { setBusy(false); }
    }}>{label("preview")}</Button>
    {identity && <><div className="font-mono text-xs break-all">{identity.algorithm} {identity.fingerprint}</div>
      <div className="flex items-center gap-2"><Checkbox id={`${id}-confirm`} disabled={busy} checked={authorized} onCheckedChange={checked => setAuthorized(checked === true)} />
        <Label htmlFor={`${id}-confirm`}>{label("authorize")}</Label></div>
      <Button type="button" disabled={busy || !authorized || !validLifetime} onClick={async () => {
        setBusy(true); setStatus("");
        try { const result = await sshAddAgentIdentity(request()); setStatus(result.already_present ? label("alreadyPresent") : label("added")); }
        catch (error) { setStatus(String(error)); } finally { setBusy(false); setPassphrase(""); setAuthorized(false); setIdentity(null); }
      }}>{label("add")}</Button></>}
    {status && <div className="text-xs break-all" role="status">{status}</div>}
  </div>;
}
