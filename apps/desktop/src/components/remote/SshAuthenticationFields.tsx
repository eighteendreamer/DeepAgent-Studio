import { canSaveSshTiming, type SshTimingForm } from "./SshTimingFields";
import { useEffect, useId, useState } from "react";
import { canSaveSshProxy, type SshProxyForm } from "./SshProxyFields";
import { useTranslation } from "react-i18next";
import { pickSshIdentityFile, sshProbeAgent, type SshAgentProbe, type SshAgentProvider, type SshAuthenticationFactor, type SshAuthenticationFactorRequest } from "../../api";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Textarea } from "../shadcn/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { canSaveSshJump, type SshJumpForm } from "./SshJumpFields";

export type SshFormAuthType = "password" | "file" | "content" | "agent" | "openssh_agent" | "pageant" | "chain";

export interface SshAuthenticationFactorForm {
  id?: string;
  authType: "password" | "file" | "content";
  keyPath: string;
  keyContent: string;
  hasKeyContent: boolean;
  certificatePath: string;
  password: string;
  hasPassword?: boolean;
  hasPassphrase?: boolean;
  clearPassphrase?: boolean;
}

export function authenticationFactorsFromDto(factors: SshAuthenticationFactor[]): SshAuthenticationFactorForm[] {
  return factors.map((factor) => ({
    id: factor.id,
    authType: factor.auth_type === "key_file" ? "file" : factor.auth_type === "key_content" ? "content" : "password",
    keyPath: factor.key_path ?? "", keyContent: "", hasKeyContent: factor.has_key_content,
    certificatePath: factor.certificate_path ?? "", password: "",
    hasPassword: factor.has_password, hasPassphrase: factor.has_passphrase,
  }));
}

export function authenticationFactorRequests(factors: SshAuthenticationFactorForm[]): SshAuthenticationFactorRequest[] {
  return factors.map((factor) => ({
    id: factor.id,
    auth_type: factor.authType === "file" ? "key_file" : factor.authType === "content" ? "key_content" : "password",
    key_path: factor.authType === "file" ? factor.keyPath : undefined,
    key_content: factor.authType === "content" ? factor.keyContent || undefined : undefined,
    certificate_path: factor.authType !== "password" ? factor.certificatePath : undefined,
    password: factor.authType === "password" ? factor.password || undefined : undefined,
    passphrase: factor.authType !== "password" ? factor.clearPassphrase ? "" : factor.password || undefined : undefined,
  }));
}

function emptyAuthenticationFactor(): SshAuthenticationFactorForm {
  return { authType: "password", password: "", keyPath: "", keyContent: "", hasKeyContent: false, certificatePath: "" };
}

interface AuthenticationValue {
  authType: SshFormAuthType;
  keyPath: string;
  keyContent: string;
  hasKeyContent: boolean;
  certificatePath: string;
  password: string;
  authenticationFactors?: SshAuthenticationFactorForm[];
  hasPassword?: boolean;
  hasPassphrase?: boolean;
  clearPassphrase?: boolean;
  jump?: SshJumpForm;
}

interface Props {
  value: AuthenticationValue;
  onChange: (change: Partial<AuthenticationValue>) => void;
  onPickKeyFile: () => void;
  onPickCertificateFile: () => void;
  factorMode?: boolean;
}

export function canSaveSshConnection(value: AuthenticationValue & { name: string; host: string; username: string; proxy?: SshProxyForm; timing?: SshTimingForm }): boolean {
  if (!canSaveSshTiming(value.timing)) return false;
  if (!canSaveSshProxy(value.proxy)) return false;
  if (!canSaveSshJump(value.jump)) return false;
  if (!value.name.trim() || !value.host.trim() || !value.username.trim()) return false;
  if (value.authType === "chain") {
    const factors = value.authenticationFactors ?? [];
    return factors.length > 0 && factors.length <= 6 && factors.every((factor) =>
      factor.authType === "password" ? Boolean(factor.password || factor.hasPassword)
        : factor.authType === "file" ? Boolean(factor.keyPath.trim())
          : Boolean(factor.keyContent.trim() || factor.hasKeyContent));
  }
  if (value.authType === "file") return Boolean(value.keyPath.trim());
  if (value.authType === "content") return Boolean(value.keyContent.trim() || value.hasKeyContent);
  return true;
}

/** Both connection editors use the same credential and local Agent inspection UI. */
export function SshAuthenticationFields({ value, onChange, onPickKeyFile, onPickCertificateFile, factorMode = false }: Props) {
  const { t } = useTranslation();
  const id = useId();
  const isPrivateKey = value.authType === "file" || value.authType === "content";
  const isAgent = !factorMode && value.authType !== "password" && value.authType !== "chain" && !isPrivateKey;
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
  const factors = value.authenticationFactors ?? [];
  const updateFactor = (index: number, change: Partial<AuthenticationValue>) => {
    onChange({ authenticationFactors: factors.map((factor, position) => {
      if (position !== index) return factor;
      const changedIdentity = (change.keyPath !== undefined && change.keyPath !== factor.keyPath)
        || (change.keyContent !== undefined && change.keyContent !== factor.keyContent);
      return { ...factor, ...(changedIdentity ? { password: "", hasPassphrase: false, clearPassphrase: false, certificatePath: "" } : {}), ...change } as SshAuthenticationFactorForm;
    }) });
  };
  const pickFactorFile = async (index: number, certificate: boolean) => {
    const path = await pickSshIdentityFile(certificate ? "certificate" : undefined);
    if (path) updateFactor(index, certificate ? { certificatePath: path } : { keyPath: path });
  };
  const moveFactor = (index: number, offset: number) => {
    const next = [...factors];
    [next[index], next[index + offset]] = [next[index + offset], next[index]];
    onChange({ authenticationFactors: next });
  };

  return <div className="space-y-3">
    <div className="space-y-1.5">
      <Label htmlFor={`${id}-method`}>{t("settings.connections.agent.authentication")}</Label>
      <Select value={isAgent ? "agent" : value.authType} onValueChange={(authType) => onChange({ authType: authType as SshFormAuthType, password: "", keyPath: "", keyContent: "", hasKeyContent: false, certificatePath: "", hasPassword: false, hasPassphrase: false, clearPassphrase: false, authenticationFactors: authType === "chain" ? [emptyAuthenticationFactor()] : [] })}>
        <SelectTrigger id={`${id}-method`}><SelectValue placeholder={isAgent ? "SSH Agent" : t(value.authType === "chain" ? "settings.connections.authenticationChain" : value.authType === "content" ? "settings.connections.privateKeyContent" : value.authType === "file" ? "settings.connections.identityFile" : "settings.connections.password")} /></SelectTrigger>
        <SelectContent>
          <SelectItem value="password">{t("settings.connections.password")}</SelectItem>
          <SelectItem value="file">{t("settings.connections.identityFile")}</SelectItem>
          <SelectItem value="content">{t("settings.connections.privateKeyContent")}</SelectItem>
          {!factorMode && <SelectItem value="agent">SSH Agent</SelectItem>}
          {!factorMode && <SelectItem value="chain">{t("settings.connections.authenticationChain")}</SelectItem>}
        </SelectContent>
      </Select>
    </div>
    {value.authType === "chain" && <div className="space-y-3">
      {factors.map((factor, index) => <div key={factor.id ?? index} className="space-y-3 rounded-xl border border-border-theme p-3">
        <div className="flex items-center justify-between gap-2">
          <Label>{t("settings.connections.authenticationFactor", { index: index + 1 })}</Label>
          <div className="flex gap-1">
            <Button type="button" variant="ghost" size="sm" disabled={index === 0} onClick={() => moveFactor(index, -1)}>{t("settings.connections.moveFactorUp")}</Button>
            <Button type="button" variant="ghost" size="sm" disabled={index === factors.length - 1} onClick={() => moveFactor(index, 1)}>{t("settings.connections.moveFactorDown")}</Button>
            <Button type="button" variant="ghost" size="sm" onClick={() => onChange({ authenticationFactors: factors.filter((_, position) => position !== index) })}>{t("settings.connections.removeFactor")}</Button>
          </div>
        </div>
        <SshAuthenticationFields factorMode value={factor} onChange={(change) => updateFactor(index, change)}
          onPickKeyFile={() => void pickFactorFile(index, false)} onPickCertificateFile={() => void pickFactorFile(index, true)} />
      </div>)}
      <Button type="button" variant="outline" disabled={factors.length >= 6} onClick={() => onChange({ authenticationFactors: [...factors, emptyAuthenticationFactor()] })}>{t("settings.connections.addFactor")}</Button>
    </div>}
    {value.authType === "password" && <div className="space-y-1.5">
      <Label htmlFor={`${id}-password`}>{t("settings.connections.password")}</Label>
      <Input id={`${id}-password`} type="password" autoComplete="new-password" value={value.password} placeholder={value.hasPassword ? t("settings.connections.savedCredential") : undefined} onChange={(event) => onChange({ password: event.target.value })} />
    </div>}
    {value.authType === "content" && <div className="space-y-1.5">
      <Label htmlFor={`${id}-content`}>{t("settings.connections.privateKeyContent")}</Label>
      <Textarea id={`${id}-content`} autoComplete="off" spellCheck={false} value={value.keyContent}
        placeholder={value.hasKeyContent ? t("settings.connections.savedPrivateKeyContent") : "-----BEGIN OPENSSH PRIVATE KEY-----"}
        onChange={(event) => onChange({ keyContent: event.target.value })} className="font-mono" />
    </div>}
    {value.authType === "file" && <div className="space-y-1.5">
      <Label htmlFor={`${id}-key`}>{t("settings.connections.identityFilePath")}</Label>
      <div className="flex items-center gap-2">
        <Input id={`${id}-key`} value={value.keyPath} onChange={(event) => onChange({ keyPath: event.target.value })} placeholder={t("settings.connections.selectIdentityFile")} />
        <Button type="button" variant="outline" className="shrink-0" onClick={onPickKeyFile}>{t("settings.connections.browse")}</Button>
      </div>
    </div>}
    {isPrivateKey && <div className="space-y-1.5">
      <Label htmlFor={`${id}-passphrase`}>{t("settings.connections.privateKeyPassphrase")}</Label>
      <Input id={`${id}-passphrase`} type="password" autoComplete="new-password" value={value.password} placeholder={value.hasPassphrase ? t("settings.connections.savedCredential") : undefined} onChange={(event) => onChange({ password: event.target.value, clearPassphrase: false })} />
      {factorMode && value.hasPassphrase && <Button type="button" variant="ghost" size="sm" onClick={() => onChange({ password: "", hasPassphrase: false, clearPassphrase: true })}>{t("settings.connections.clearSavedPassphrase")}</Button>}
    </div>}
    {isPrivateKey && <div className="space-y-1.5">
      <Label htmlFor={`${id}-certificate`}>{t("settings.connections.userCertificate")}</Label>
      <div className="flex items-center gap-2">
        <Input id={`${id}-certificate`} value={value.certificatePath} onChange={(event) => onChange({ certificatePath: event.target.value })} placeholder={t("settings.connections.selectUserCertificate")} />
        <Button type="button" variant="outline" className="shrink-0" onClick={onPickCertificateFile}>{t("settings.connections.browse")}</Button>
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
