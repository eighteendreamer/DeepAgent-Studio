import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { pickSshIdentityFile, sshPreviewConfigImport, type SshConfigImportPreview, type SshConfigImportedConnection, type SshConfigImportedEndpoint, type SshConfigOverrides } from "../../api";
import type { SshAuthenticationFactorForm, SshFormAuthType } from "./SshAuthenticationFields";
import type { SshJumpForm } from "./SshJumpFields";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";

export interface SshConfigImportForm {
  name: string;
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
  jump: SshJumpForm;
  importPortExplicit?: boolean;
  importAuthExplicit?: boolean;
  importJumpExplicit?: boolean;
}

export function sshConfigOverridesFromForm(form: SshConfigImportForm): SshConfigOverrides {
  const explicitPort = form.importPortExplicit || form.port !== "22";
  const port = Number(form.port);
  if (explicitPort && (!/^\d+$/.test(form.port.trim()) || !Number.isInteger(port) || port < 1 || port > 65535)) {
    throw new Error("SSH port must be an integer between 1 and 65535.");
  }
  return {
    host: form.host.trim() || undefined,
    username: form.username.trim() || undefined,
    port: explicitPort ? port : undefined,
    identity_files: form.importAuthExplicit ? [] : undefined,
    proxy_jump: form.importJumpExplicit ? "none" : undefined,
  };
}

type Authentication = Pick<SshConfigImportForm, "authType" | "keyPath" | "keyContent" | "hasKeyContent" | "certificatePath" | "password" | "authenticationFactors" | "agentFingerprint">;

function importedAuthentication(endpoint: SshConfigImportedEndpoint): Authentication {
  const paths = endpoint.identity_files;
  return {
    authType: paths.length > 1 ? "chain" : paths.length === 1 ? "file" : endpoint.default_identities ? "auto_public_key" : "agent",
    agentFingerprint: "", keyPath: paths.length === 1 ? paths[0] : "",
    keyContent: "", hasKeyContent: false, certificatePath: "", password: "",
    authenticationFactors: paths.length > 1 ? paths.map((keyPath) => ({
      authType: "file", keyPath, keyContent: "", hasKeyContent: false, certificatePath: "", password: "",
    })) : [],
  };
}

function clearImportedSecrets<T extends Authentication>(value: T): T {
  return {
    ...value, password: "", hasPassword: false, hasPassphrase: false, clearPassphrase: false,
    keyContent: "", hasKeyContent: false, certificatePath: "",
    authenticationFactors: value.authenticationFactors.map((factor) => ({
      ...factor, id: undefined, password: "", hasPassword: false, hasPassphrase: false,
      clearPassphrase: false, keyContent: "", hasKeyContent: false, certificatePath: "",
    })),
  };
}

export function applySshConfigImport<T extends SshConfigImportForm>(form: T, connection: SshConfigImportedConnection): T {
  const target = connection.target;
  const sameTarget = form.host === target.host && Number(form.port) === target.port && form.username === target.username;
  const jump = form.importJumpExplicit ? form.jump : connection.jump ? {
    ...form.jump, ...importedAuthentication(connection.jump), enabled: true,
    host: connection.jump.host, port: String(connection.jump.port), username: connection.jump.username,
    hasPassword: false, hasPassphrase: false, clearPassphrase: false,
  } : { ...form.jump, enabled: false };
  const sameRoute = form.jump.enabled === jump.enabled && (!jump.enabled || (
    form.jump.host === jump.host && form.jump.port === jump.port && form.jump.username === jump.username
  ));
  const authentication = form.importAuthExplicit ? form : importedAuthentication(target);
  return {
    ...form,
    ...(sameTarget && sameRoute ? authentication : clearImportedSecrets(authentication)),
    name: form.name || connection.alias, host: target.host, port: String(target.port), username: target.username,
    jump, importPortExplicit: true,
  };
}

export function SshConfigImporter<T extends SshConfigImportForm>({ value, onApply }: { value: T; onApply: (value: T) => void }) {
  const { t } = useTranslation();
  const label = (key: string) => t(`settings.connections.configImport.${key}`);
  const id = useId();
  const [path, setPath] = useState("");
  const [alias, setAlias] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ key: string; message: string } | null>(null);
  const [report, setReport] = useState<{ key: string; source: string; preview: SshConfigImportPreview } | null>(null);
  const revision = useRef(0);
  const key = JSON.stringify([path.trim(), alias.trim(), value]);
  const latestKey = useRef(key);
  latestKey.current = key;
  useEffect(() => () => { revision.current += 1; }, []);
  const preview = async () => {
    const request = ++revision.current;
    setBusy(true); setError(null); setReport(null);
    try {
      const result = await sshPreviewConfigImport(path.trim() || undefined, alias.trim() || undefined, sshConfigOverridesFromForm(value));
      if (request === revision.current && key === latestKey.current) setReport({ key, source: path.trim(), preview: result });
    } catch (error) {
      if (request === revision.current && key === latestKey.current) setError({ key, message: String(error) });
    } finally {
      if (request === revision.current) setBusy(false);
    }
  };
  const pick = async () => {
    const request = ++revision.current;
    setBusy(true); setError(null);
    try {
      const selected = await pickSshIdentityFile("config");
      if (selected && request === revision.current && key === latestKey.current) setPath(selected);
    } catch (error) {
      if (request === revision.current && key === latestKey.current) setError({ key, message: String(error) });
    } finally {
      if (request === revision.current) setBusy(false);
    }
  };
  const current = report?.key === key ? report.preview.connection : undefined;
  const candidate = current ? applySshConfigImport(value, current) : undefined;
  const aliases = report?.source === path.trim() ? report.preview.aliases : [];
  const authLabel = (authType: SshFormAuthType) => authType === "openssh_agent" ? "OpenSSH Agent" : authType === "pageant" ? "Pageant" :
    t(`settings.connections.${authType === "auto_public_key" ? "agent.autoPublicKey" : authType === "agent" ? "agent.auto"
      : authType === "file" ? "identityFile" : authType === "content" ? "privateKeyContent" : authType === "chain" ? "authenticationChain" : "password"}`);
  return <section className="space-y-2.5 rounded-xl border border-border-theme p-3">
    <Label htmlFor={`${id}-path`}>{label("title")}</Label>
    <div className="flex gap-2">
      <Input id={`${id}-path`} value={path} placeholder="~/.ssh/config" onChange={(event) => setPath(event.target.value)} />
      <Button type="button" variant="outline" disabled={busy} onClick={() => void pick()}>{label("browse")}</Button>
    </div>
    <Label htmlFor={`${id}-alias`}>{label("alias")}</Label>
    <Input id={`${id}-alias`} value={alias} onChange={(event) => setAlias(event.target.value)} />
    {aliases.length > 0 && <Select value={aliases.includes(alias) ? alias : ""} onValueChange={setAlias}>
      <SelectTrigger aria-label={label("chooseAlias")}><SelectValue placeholder={aliases.includes(alias) ? alias : label("chooseAlias")} /></SelectTrigger>
      <SelectContent>{aliases.map((name) => <SelectItem key={name} value={name}>{name}</SelectItem>)}</SelectContent>
    </Select>}
    <Button type="button" variant="outline" disabled={busy} onClick={() => void preview()}>{label(busy ? "loading" : "preview")}</Button>
    <div role="status" aria-live="polite">
      {error?.key === key && <p className="break-words text-red-600">{error.message}</p>}
      {report?.key === key && !current && aliases.length === 0 && <p>{label("noAliases")}</p>}
    </div>
    {candidate && <div className="space-y-2 break-words">
      <Label>{label("effective")}</Label>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
        <dt>{t("settings.connections.hostname")}</dt><dd>{candidate.host}</dd>
        <dt>{t("settings.connections.sshPort")}</dt><dd>{candidate.port}</dd>
        <dt>{t("settings.connections.username")}</dt><dd>{candidate.username}</dd>
        <dt>{label("authentication")}</dt><dd>{authLabel(candidate.authType)}</dd>
        <dt>IdentityFile</dt><dd>{candidate.authType === "file" ? candidate.keyPath : candidate.authType === "chain" ? candidate.authenticationFactors.filter((factor) => factor.authType === "file").map((factor) => factor.keyPath).join(" · ") : "—"}</dd>
        <dt>ProxyJump</dt><dd>{candidate.jump.enabled ? `${candidate.jump.username}@${candidate.jump.host}:${candidate.jump.port}` : "—"}</dd>
      </dl>
      {candidate.jump.enabled && <div>{authLabel(candidate.jump.authType)} · {candidate.jump.authType === "chain" ? candidate.jump.authenticationFactors.map((factor) => factor.keyPath).join(" · ") : candidate.jump.keyPath}</div>}
      <Button type="button" disabled={busy} onClick={() => { if (report?.key === latestKey.current) onApply(candidate); }}>{label("apply")}</Button>
    </div>}
  </section>;
}
