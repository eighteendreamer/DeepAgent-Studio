import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshProbeAgent, type SshAgentProbe, type SshAgentProvider } from "../../api";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";

export type SshFormAuthType = "password" | "file" | "agent" | "openssh_agent" | "pageant";

interface AuthenticationValue {
  authType: SshFormAuthType;
  keyPath: string;
  password: string;
}

interface Props {
  value: AuthenticationValue;
  onChange: (change: Partial<AuthenticationValue>) => void;
  onPickKeyFile: () => void;
}

/** Both connection editors use the same credential and local Agent inspection UI. */
export function SshAuthenticationFields({ value, onChange, onPickKeyFile }: Props) {
  const { t } = useTranslation();
  const id = useId();
  const isAgent = value.authType !== "password" && value.authType !== "file";
  const provider: SshAgentProvider = value.authType === "pageant" ? "pageant" : value.authType === "openssh_agent" ? "open_ssh" : "auto";
  const [refresh, setRefresh] = useState(0);
  const [report, setReport] = useState<{ provider: SshAgentProvider; refresh: number; probes?: SshAgentProbe[]; error?: string } | null>(null);

  useEffect(() => {
    setReport(null);
    if (!isAgent) return;
    let current = true;
    sshProbeAgent(provider).then(
      (probes) => { if (current) setReport({ provider, refresh, probes }); },
      (error: unknown) => { if (current) setReport({ provider, refresh, error: String(error) }); },
    );
    return () => { current = false; };
  }, [isAgent, provider, refresh]);

  const current = report?.provider === provider && report.refresh === refresh ? report : null;

  return <div className="space-y-3">
    <div className="space-y-1.5">
      <Label htmlFor={`${id}-method`}>{t("settings.connections.agent.authentication")}</Label>
      <Select value={isAgent ? "agent" : value.authType} onValueChange={(authType) => onChange({ authType: authType as SshFormAuthType, password: "", keyPath: "" })}>
        <SelectTrigger id={`${id}-method`}><SelectValue placeholder={isAgent ? "SSH Agent" : t(value.authType === "file" ? "settings.connections.identityFile" : "settings.connections.password")} /></SelectTrigger>
        <SelectContent>
          <SelectItem value="password">{t("settings.connections.password")}</SelectItem>
          <SelectItem value="file">{t("settings.connections.identityFile")}</SelectItem>
          <SelectItem value="agent">SSH Agent</SelectItem>
        </SelectContent>
      </Select>
    </div>
    {value.authType === "password" && <div className="space-y-1.5">
      <Label htmlFor={`${id}-password`}>{t("settings.connections.password")}</Label>
      <Input id={`${id}-password`} type="password" autoComplete="new-password" value={value.password} onChange={(event) => onChange({ password: event.target.value })} />
    </div>}
    {value.authType === "file" && <div className="space-y-1.5">
      <Label htmlFor={`${id}-key`}>{t("settings.connections.identityFilePath")}</Label>
      <div className="flex items-center gap-2">
        <Input id={`${id}-key`} value={value.keyPath} onChange={(event) => onChange({ keyPath: event.target.value })} placeholder={t("settings.connections.selectIdentityFile")} />
        <Button type="button" variant="outline" className="shrink-0" onClick={onPickKeyFile}>{t("settings.connections.browse")}</Button>
      </div>
    </div>}
    {isAgent && <div className="space-y-2 rounded-xl border border-border-theme p-3">
      <Label htmlFor={`${id}-provider`}>{t("settings.connections.agent.provider")}</Label>
      <Select value={provider} onValueChange={(next) => onChange({ authType: next === "pageant" ? "pageant" : next === "open_ssh" ? "openssh_agent" : "agent" })}>
        <SelectTrigger id={`${id}-provider`}><SelectValue placeholder={provider === "auto" ? t("settings.connections.agent.auto") : provider === "pageant" ? "Pageant" : "OpenSSH Agent"} /></SelectTrigger>
        <SelectContent>
          <SelectItem value="auto">{t("settings.connections.agent.auto")}</SelectItem>
          <SelectItem value="open_ssh">OpenSSH Agent</SelectItem>
          <SelectItem value="pageant">Pageant</SelectItem>
        </SelectContent>
      </Select>
      <div role="status" aria-live="polite" className="space-y-2 text-[12px]">
        {!current && <p>{t("settings.connections.agent.checking")}</p>}
        {current?.error && <p className="break-words text-red-600">{current.error}</p>}
        {current?.probes?.map((probe) => <div key={probe.provider} className="space-y-1">
          <p className="font-medium">{probe.provider === "pageant" ? "Pageant" : "OpenSSH Agent"} · {t(`settings.connections.agent.states.${probe.state}`)}</p>
          {probe.reason && <p className="break-words text-text-secondary">{probe.reason}</p>}
          {probe.identities.map((identity) => <p key={identity.fingerprint} className="break-all font-mono text-[11px]">{identity.algorithm} {identity.fingerprint}</p>)}
        </div>)}
      </div>
      <Button type="button" variant="outline" size="sm" disabled={!current} onClick={() => setRefresh((previous) => previous + 1)}>{t("settings.connections.agent.refresh")}</Button>
    </div>}
  </div>;
}
