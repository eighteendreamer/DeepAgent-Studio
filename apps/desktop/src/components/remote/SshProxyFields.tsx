import { useId } from "react";
import { useTranslation } from "react-i18next";
import type { SshJumpEndpoint, SshProxy, SshProxyRequest } from "../../api";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";

export interface SshProxyForm {
  mode: "direct" | "http_connect" | "socks5";
  host: string;
  port: string;
  username: string;
  password: string;
  hasPassword: boolean;
  clearPassword?: boolean;
}

export function sshProxyFromDto(proxy?: SshProxy | null): SshProxyForm {
  return { mode: proxy?.mode ?? "direct", host: proxy?.host ?? "", port: String(proxy?.port ?? 8080),
    username: proxy?.username ?? "", password: "", hasPassword: proxy?.has_password ?? false };
}

export function sshProxyRequest(proxy?: SshProxyForm): SshProxyRequest {
  if (!proxy || proxy.mode === "direct") return { mode: "direct" };
  return { mode: proxy.mode, host: proxy.host.trim(), port: Number(proxy.port),
    username: proxy.username || undefined, password: proxy.clearPassword ? "" : proxy.password || undefined };
}

export function canSaveSshProxy(proxy?: SshProxyForm): boolean {
  if (!proxy || proxy.mode === "direct") return true;
  const port = Number(proxy.port);
  if (!proxy.host.trim() || /[\s/\\]/.test(proxy.host.trim()) || !Number.isInteger(port) || port < 1 || port > 65535) return false;
  if (!proxy.username && proxy.password) return false;
  if (/[\x00-\x1f\x7f]/.test(proxy.username)) return false;
  if (proxy.mode === "http_connect") return !proxy.username.includes(":");
  const bytes = new TextEncoder();
  return !proxy.username || (bytes.encode(proxy.username).length <= 255
    && ((proxy.hasPassword && !proxy.clearPassword && !proxy.password) || (bytes.encode(proxy.password).length > 0 && bytes.encode(proxy.password).length <= 255)));
}

export function isSshRoute(result: { route: string; proxy?: Pick<SshProxy, "mode" | "host" | "port"> | null; jump?: SshJumpEndpoint | null }): boolean {
  if (result.route === "direct") return result.proxy == null && result.jump == null;
  if (result.route === "jump") {
    const jump = result.jump;
    return Boolean(jump?.host && jump.username) && Number.isInteger(jump?.port) && jump!.port > 0 && jump!.port <= 65535
      && (result.proxy == null || ((result.proxy.mode === "http_connect" || result.proxy.mode === "socks5")
        && Boolean(result.proxy.host) && Number.isInteger(result.proxy.port) && result.proxy.port > 0 && result.proxy.port <= 65535));
  }
  if (result.jump != null) return false;
  return (result.route === "http_connect" || result.route === "socks5")
    && result.proxy?.mode === result.route && Boolean(result.proxy.host)
    && Number.isInteger(result.proxy.port) && result.proxy.port > 0 && result.proxy.port <= 65535;
}

export function matchesSshRoute(connection: { proxy?: SshProxy | null; jump?: SshJumpEndpoint | null }, result: { route: string; proxy?: Pick<SshProxy, "mode" | "host" | "port"> | null; jump?: SshJumpEndpoint | null }): boolean {
  const expected = connection.proxy;
  const jump = connection.jump;
  return isSshRoute(result) && result.route === (jump ? "jump" : expected?.mode ?? "direct")
    && (jump ? result.jump?.host === jump.host && result.jump.port === jump.port && result.jump.username === jump.username : result.jump == null) && (expected
    ? result.proxy?.mode === expected.mode && result.proxy.host === expected.host && result.proxy.port === expected.port
    : result.proxy == null);
}

export function SshRouteLabel({ proxy, jump }: { proxy?: Pick<SshProxy, "mode" | "host" | "port"> | null; jump?: SshJumpEndpoint | null }) {
  const { t } = useTranslation();
  const proxyRoute = proxy ? `${proxy.mode === "http_connect" ? "HTTP CONNECT" : "SOCKS5"} · ${proxy.host}:${proxy.port}` : "";
  return <>{[proxyRoute, jump ? `SSH · ${jump.username}@${jump.host}:${jump.port}` : ""].filter(Boolean).join(" → ") || t("settings.connections.proxy.direct")}</>;
}

export function SshProxyFields({ value, onChange }: { value: SshProxyForm; onChange: (value: SshProxyForm) => void }) {
  const id = useId();
  const { t } = useTranslation();
  const label = (key: string) => t(`settings.connections.proxy.${key}`);
  const identity = (change: Partial<SshProxyForm>) => onChange({ ...value, ...change, password: "", hasPassword: false, clearPassword: false });
  return <div className="space-y-3">
    <div className="space-y-1.5">
      <Label htmlFor={`${id}-mode`}>{label("route")}</Label>
      <Select value={value.mode} onValueChange={mode => identity({ mode: mode as SshProxyForm["mode"], port: mode === "socks5" ? "1080" : "8080" })}>
        <SelectTrigger id={`${id}-mode`}><SelectValue placeholder={value.mode === "direct" ? label("direct") : value.mode === "socks5" ? "SOCKS5" : "HTTP CONNECT"} /></SelectTrigger>
        <SelectContent><SelectItem value="direct">{label("direct")}</SelectItem><SelectItem value="http_connect">HTTP CONNECT</SelectItem><SelectItem value="socks5">SOCKS5</SelectItem></SelectContent>
      </Select>
    </div>
    {value.mode !== "direct" && <>
      <div className="space-y-1.5"><Label htmlFor={`${id}-host`}>{label("host")}</Label>
        <Input id={`${id}-host`} value={value.host} onChange={event => identity({ host: event.target.value })} placeholder="proxy.example.com" autoComplete="off" /></div>
      <div className="space-y-1.5"><Label htmlFor={`${id}-port`}>{label("port")}</Label>
        <Input id={`${id}-port`} value={value.port} onChange={event => identity({ port: event.target.value })} inputMode="numeric" /></div>
      <div className="space-y-1.5"><Label htmlFor={`${id}-username`}>{label("username")}</Label>
        <Input id={`${id}-username`} value={value.username} onChange={event => identity({ username: event.target.value })} autoComplete="off" /></div>
      <div className="space-y-1.5"><Label htmlFor={`${id}-password`}>{label("password")}</Label>
        <Input id={`${id}-password`} value={value.password} onChange={event => onChange({ ...value, password: event.target.value, clearPassword: false })}
          placeholder={value.hasPassword ? t("settings.connections.savedCredential") : undefined} type="password" autoComplete="new-password" />
        {value.hasPassword && <Button type="button" variant="ghost" size="sm" onClick={() => onChange({ ...value, password: "", hasPassword: false, clearPassword: true })}>{label("clearPassword")}</Button>}
      </div>
    </>}
  </div>;
}
