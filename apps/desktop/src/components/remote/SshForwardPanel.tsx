import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshConnectionDetails, sshListForwards, sshStartForward, sshStopForward, type SshConnectionDetails, type SshForwardRequest, type SshForwardTask } from "../../api";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";

export interface ForwardForm { kind: SshForwardRequest["kind"]; bindHost: string; bindPort: string; targetHost: string; targetPort: string; allowNonLoopback: boolean }

export function forwardingRequest(form: ForwardForm): SshForwardRequest | null {
  const port = (value: string, zero: boolean) => /^\d+$/.test(value) && Number(value) >= (zero ? 0 : 1) && Number(value) <= 65535;
  const host = form.bindHost.trim();
  if (!host || !port(form.bindPort, true)) return null;
  if (!form.allowNonLoopback && host !== "::1" && !/^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host)) return null;
  if (form.kind !== "dynamic" && (!form.targetHost.trim() || /\s/.test(form.targetHost) || !port(form.targetPort, false))) return null;
  return { kind: form.kind, bind_host: host, bind_port: Number(form.bindPort),
    target_host: form.kind === "dynamic" ? null : form.targetHost.trim(), target_port: form.kind === "dynamic" ? null : Number(form.targetPort), allow_non_loopback: form.allowNonLoopback };
}

export function SshForwardPanel({ connectionId }: { connectionId: string }) {
  const { t } = useTranslation(); const label = (key: string) => t(`remote.forwarding.${key}`);
  const [details, setDetails] = useState<SshConnectionDetails | null>(null);
  const [tasks, setTasks] = useState<SshForwardTask[]>([]); const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  const [form, setForm] = useState<ForwardForm>({ kind: "local", bindHost: "127.0.0.1", bindPort: "0", targetHost: "127.0.0.1", targetPort: "22", allowNonLoopback: false });
  const active = useRef(true); const revision = useRef(0);
  const refresh = async () => {
    const round = ++revision.current;
    const next = await sshConnectionDetails(connectionId);
    const jobs = await sshListForwards(connectionId, next.transport_generation);
    if (active.current && revision.current === round) { setDetails(next); setTasks(jobs); }
  };
  useEffect(() => {
    active.current = true; let cancelled = false; let timeout: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { await refresh(); } catch (reason) { if (!cancelled) { setDetails(null); setTasks([]); setError(String(reason)); } }
      if (!cancelled) timeout = setTimeout(() => void poll(), 2000);
    };
    void poll();
    return () => { cancelled = true; active.current = false; ++revision.current; clearTimeout(timeout); };
  }, [connectionId]);
  const patch = (change: Partial<ForwardForm>) => setForm(previous => ({ ...previous, ...change }));
  const request = forwardingRequest(form);
  return <div className="max-h-[55vh] shrink-0 space-y-3 overflow-auto border-b border-border-theme bg-surface px-4 py-3" data-ssh-forwarding="true">
    <div className="text-sm font-medium">{label("title")}</div>
    {details?.algorithms && <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
      {Object.entries(details.algorithms).map(([key, value]) => <div key={key} className="flex gap-2"><dt className="text-text-secondary">{label(key)}</dt><dd className="break-all font-mono">{value}</dd></div>)}
    </dl>}
    <div className="grid grid-cols-3 gap-3">
      <div className="space-y-1"><Label htmlFor="ssh-forward-kind">{label("kind")}</Label><Select value={form.kind} onValueChange={kind => patch({ kind: kind as ForwardForm["kind"] })}>
        <SelectTrigger id="ssh-forward-kind"><SelectValue /></SelectTrigger><SelectContent>{(["local", "remote", "dynamic"] as const).map(kind => <SelectItem key={kind} value={kind}>{label(kind)}</SelectItem>)}</SelectContent></Select></div>
      <div className="space-y-1"><Label htmlFor="ssh-forward-bind">{label("bindHost")}</Label><Input id="ssh-forward-bind" value={form.bindHost} onChange={event => patch({ bindHost: event.target.value, allowNonLoopback: false })} /></div>
      <div className="space-y-1"><Label htmlFor="ssh-forward-port">{label("bindPort")}</Label><Input id="ssh-forward-port" type="number" min={0} max={65535} value={form.bindPort} onChange={event => patch({ bindPort: event.target.value })} /></div>
      {form.kind !== "dynamic" && <><div className="space-y-1"><Label htmlFor="ssh-forward-target">{label("targetHost")}</Label><Input id="ssh-forward-target" value={form.targetHost} onChange={event => patch({ targetHost: event.target.value })} /></div>
        <div className="space-y-1"><Label htmlFor="ssh-forward-target-port">{label("targetPort")}</Label><Input id="ssh-forward-target-port" type="number" min={1} max={65535} value={form.targetPort} onChange={event => patch({ targetPort: event.target.value })} /></div></>}
    </div>
    <div className="flex flex-wrap items-center gap-3"><Checkbox id="ssh-forward-expose" checked={form.allowNonLoopback} onCheckedChange={checked => patch({ allowNonLoopback: checked === true })} />
      <Label htmlFor="ssh-forward-expose">{label("expose")}</Label>
      <Button type="button" disabled={busy || !request || !details} onClick={async () => {
        if (!request || !details) return; setBusy(true); setError("");
        try { await sshStartForward(connectionId, details.transport_generation, request); await refresh(); }
        catch (reason) { if (active.current) setError(String(reason)); } finally { if (active.current) setBusy(false); }
      }}>{label("start")}</Button></div>
    {form.kind === "dynamic" && <div className="text-xs text-text-secondary">{label("remoteDns")}</div>}
    {error && <div role="alert" className="break-all text-xs text-red-500">{error}</div>}
    <div className="space-y-2">{tasks.map(task => <div key={task.id} className="space-y-1 rounded-md border border-border-theme p-2 text-xs">
      <div className="flex items-center gap-3"><span>{label(task.kind)}</span><span className="font-mono">{task.bind_host}:{task.bind_port} {task.target_host ? `→ ${task.target_host}:${task.target_port}` : "SOCKS5"}</span>
        <span>{label(task.status)}</span><span>{label("channels")}: {task.active_channels}</span>
        {(task.status === "running" || task.status === "error") && <Button type="button" variant="outline" size="sm" disabled={busy || !details} onClick={async () => {
          if (!details) return; setBusy(true); setError("");
          try { await sshStopForward(connectionId, task.transport_generation, task.id); await refresh(); }
          catch (reason) { if (active.current) setError(String(reason)); } finally { if (active.current) setBusy(false); }
        }}>{label("stop")}</Button>}</div>
      <div className="font-mono">↑ {task.bytes_to_target} B　↓ {task.bytes_from_target} B</div>
      <details><summary>{label("events")}</summary>{task.events.map((event, index) => <div key={index} className="break-all">{event}</div>)}</details>
    </div>)}</div>
  </div>;
}
