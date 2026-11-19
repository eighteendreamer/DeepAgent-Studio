import { useId } from "react";
import { useTranslation } from "react-i18next";
import type { SshJump, SshJumpRequest, SshHostKeyPolicy } from "../../api";
import { pickSshIdentityFile } from "../../api";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { SshHostTrustField } from "./SshHostTrustField";
import {
  authenticationFactorRequests,
  authenticationFactorsFromDto,
  SshAuthenticationFields,
  type SshAuthenticationFactorForm,
  type SshFormAuthType,
} from "./SshAuthenticationFields";

export interface SshJumpForm {
  enabled: boolean;
  host: string;
  port: string;
  username: string;
  authType: SshFormAuthType;
  agentFingerprint?: string;
  keyPath: string;
  keyContent: string;
  hasKeyContent: boolean;
  certificatePath: string;
  password: string;
  hasPassword?: boolean;
  hasPassphrase?: boolean;
  clearPassphrase?: boolean;
  authenticationFactors: SshAuthenticationFactorForm[];
  knownHostsPath: string;
  hostKeyPolicy: SshHostKeyPolicy;
}

export function sshJumpFromDto(jump?: SshJump | null): SshJumpForm {
  const authType = jump?.auth_type === "key_content" ? "content"
    : jump?.auth_type === "key_file" ? "file" : jump?.auth_type ?? "password";
  return {
    enabled: Boolean(jump),
    host: jump?.host ?? "",
    port: String(jump?.port ?? 22),
    username: jump?.username ?? "",
    authType,
    agentFingerprint: jump?.agent_fingerprint ?? "",
    keyPath: jump?.key_path ?? "",
    keyContent: "",
    hasKeyContent: jump?.has_key_content ?? false,
    certificatePath: jump?.certificate_path ?? "",
    password: "",
    hasPassword: jump?.has_password ?? false,
    hasPassphrase: jump?.has_password ?? false,
    authenticationFactors: authenticationFactorsFromDto(jump?.authentication_factors ?? []),
    knownHostsPath: jump?.known_hosts_path ?? "",
    hostKeyPolicy: jump?.host_key_policy ?? "strict",
  };
}

export function sshJumpRequest(value?: SshJumpForm): SshJumpRequest {
  if (!value || !value.enabled) return { mode: "direct" };
  const authType = value.authType === "content" ? "key_content"
    : value.authType === "file" ? "key_file" : value.authType;
  return {
    mode: "ssh",
    host: value.host.trim(),
    port: Number(value.port),
    username: value.username.trim(),
    auth_type: authType,
    agent_fingerprint: ["agent", "openssh_agent", "pageant"].includes(value.authType) ? value.agentFingerprint ?? "" : undefined,
    key_path: value.authType === "file" ? value.keyPath.trim() || undefined : undefined,
    key_content: value.authType === "content" ? value.keyContent || undefined : undefined,
    certificate_path: value.authType === "file" || value.authType === "content" ? value.certificatePath.trim() || undefined : undefined,
    password: ["password", "file", "content"].includes(value.authType)
      ? (value.clearPassphrase ? "" : value.password || undefined) : undefined,
    authentication_factors: value.authType === "chain" ? authenticationFactorRequests(value.authenticationFactors) : undefined,
    known_hosts_path: value.knownHostsPath.trim() || undefined,
    host_key_policy: value.hostKeyPolicy,
  };
}

export function canSaveSshJump(value?: SshJumpForm): boolean {
  if (!value || !value.enabled) return true;
  const port = Number(value.port);
  if (!value.host.trim() || /[\s/\\]/.test(value.host.trim()) || !Number.isInteger(port) || port < 1 || port > 65535) return false;
  if (!value.username.trim() || /[\x00-\x1f\x7f]/.test(value.username)) return false;
  if (value.authType === "chain") {
    const factors = value.authenticationFactors;
    return factors.length > 0 && factors.length <= 6 && factors.every((factor) =>
      factor.authType === "password" ? Boolean(factor.password || factor.hasPassword)
        : factor.authType === "file" ? Boolean(factor.keyPath.trim())
          : Boolean(factor.keyContent.trim() || factor.hasKeyContent));
  }
  if (value.authType === "file") return Boolean(value.keyPath.trim());
  if (value.authType === "content") return Boolean(value.keyContent.trim() || value.hasKeyContent);
  if (value.authType === "password") return Boolean(value.password || value.hasPassword);
  return true;
}

export function SshJumpFields({ value, onChange }: { value: SshJumpForm; onChange: (value: SshJumpForm) => void }) {
  const id = useId();
  const { t } = useTranslation();
  const label = (key: string) => t(`settings.connections.jump.${key}`);
  const update = (change: Partial<SshJumpForm>) => {
    const identityChanged = ["host", "port", "username", "authType", "keyPath", "keyContent", "certificatePath"]
      .some((key) => key in change && change[key as keyof SshJumpForm] !== value[key as keyof SshJumpForm]);
    onChange(identityChanged ? {
      ...value,
      password: "", hasPassword: false, hasPassphrase: false, clearPassphrase: false,
      keyContent: "", hasKeyContent: false, certificatePath: "",
      authenticationFactors: value.authenticationFactors.map((factor) => ({
        ...factor, id: undefined, password: "", hasPassword: false, hasPassphrase: false, clearPassphrase: false,
        keyContent: "", hasKeyContent: false, certificatePath: "",
      })), ...change,
    } : { ...value, ...change });
  };
  if (!value.enabled) {
    return <Button type="button" variant="outline" onClick={() => onChange({ ...value, enabled: true })}>{label("add")}</Button>;
  }
  return <div className="space-y-3" data-ssh-jump="enabled">
    <div className="flex items-center justify-between gap-2">
      <Label>{label("title")}</Label>
      <Button type="button" variant="ghost" size="sm" onClick={() => onChange({ ...value, enabled: false })}>{label("remove")}</Button>
    </div>
    <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_110px]">
      <div className="space-y-1.5"><Label htmlFor={`${id}-host`}>{label("host")}</Label><Input id={`${id}-host`} value={value.host} onChange={(event) => update({ host: event.target.value })} autoComplete="off" /></div>
      <div className="space-y-1.5"><Label htmlFor={`${id}-port`}>{label("port")}</Label><Input id={`${id}-port`} value={value.port} onChange={(event) => update({ port: event.target.value })} inputMode="numeric" /></div>
    </div>
    <div className="space-y-1.5"><Label htmlFor={`${id}-username`}>{label("username")}</Label><Input id={`${id}-username`} value={value.username} onChange={(event) => update({ username: event.target.value })} autoComplete="off" /></div>
    <SshHostTrustField
      id={`${id}-known-hosts`}
      host={value.host}
      port={Number(value.port)}
      value={value.knownHostsPath}
      policy={value.hostKeyPolicy}
      onPolicyChange={(hostKeyPolicy) => update({ hostKeyPolicy })}
      onChange={(knownHostsPath) => update({ knownHostsPath })}
    />
    <SshAuthenticationFields
      value={value}
      factorMode={false}
      onChange={(change) => update(change as Partial<SshJumpForm>)}
      onPickKeyFile={() => { void pickSshIdentityFile().then((path) => { if (path) update({ keyPath: path }); }); }}
      onPickCertificateFile={() => { void pickSshIdentityFile("certificate").then((path) => { if (path) update({ certificatePath: path }); }); }}
    />
  </div>;
}
