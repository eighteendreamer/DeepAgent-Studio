import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshDiscoverDefaultIdentities, type SshDefaultIdentity } from "../../api";
import { Button } from "../shadcn/button";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";

interface Props {
  value: string;
  onSelect: (path: string) => void;
}

type DiscoveryResult = {
  status: "idle" | "loading" | "complete" | "error";
  identities: SshDefaultIdentity[];
  error?: string;
};

export function SshDefaultIdentityPicker({ value, onSelect }: Props) {
  const { t } = useTranslation();
  const id = useId();
  const request = useRef(0);
  const [result, setResult] = useState<DiscoveryResult>({ status: "idle", identities: [] });
  useEffect(() => () => { request.current += 1; }, []);

  const discover = async () => {
    const revision = ++request.current;
    setResult({ status: "loading", identities: [] });
    try {
      const identities = await sshDiscoverDefaultIdentities();
      if (revision === request.current) setResult({ status: "complete", identities });
    } catch (error) {
      if (revision === request.current) setResult({ status: "error", identities: [], error: String(error) });
    }
  };
  const available = result.identities.filter((identity) => identity.state === "available");
  const selected = available.some((identity) => identity.path === value) ? value : "";

  return <div className="space-y-2">
    <Button type="button" variant="outline" size="sm" disabled={result.status === "loading"} onClick={discover}>
      {t(result.status === "loading" ? "settings.connections.discoveringIdentities" : "settings.connections.discoverIdentities")}
    </Button>
    {available.length > 0 && <div className="space-y-1.5">
      <Label htmlFor={id}>{t("settings.connections.defaultIdentity")}</Label>
      <Select value={selected} onValueChange={(path) => {
        if (available.some((identity) => identity.path === path)) onSelect(path);
      }}>
        <SelectTrigger id={id}><SelectValue placeholder={t("settings.connections.chooseDefaultIdentity")} /></SelectTrigger>
        <SelectContent>{available.map((identity) =>
          <SelectItem key={identity.path} value={identity.path}>{identity.name} · {identity.path}</SelectItem>,
        )}</SelectContent>
      </Select>
    </div>}
    <div role="status" aria-live="polite" className="space-y-1 break-words">
      {result.status === "complete" && available.length === 0 && <p>{t("settings.connections.noDefaultIdentities")}</p>}
      {result.error && <p className="text-red-600">{result.error}</p>}
      {result.identities.filter((identity) => identity.state === "unavailable").map((identity) =>
        <p key={identity.path} className="text-red-600">{`${identity.name}: ${identity.reason ?? t("settings.connections.identityUnavailable")}`}</p>,
      )}
    </div>
  </div>;
}
