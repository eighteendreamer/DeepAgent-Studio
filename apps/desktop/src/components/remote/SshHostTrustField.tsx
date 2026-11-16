import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshInspectHostTrust, type SshHostTrustInspection, type SshHostKeyPolicy } from "../../api";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";

/** Shared configuration; local trust inspection stays read-only under either policy. */
export function SshHostTrustField({ id, host, port, value, onChange, policy = "strict", onPolicyChange }: {
  id: string;
  host: string;
  port: number;
  value: string;
  onChange: (value: string) => void;
  policy?: SshHostKeyPolicy;
  onPolicyChange?: (policy: SshHostKeyPolicy) => void;
}) {
  const { t } = useTranslation();
  const generation = useRef(0);
  const inFlight = useRef(false);
  const scope = JSON.stringify([host, port, value]);
  const [snapshot, setInspection] = useState<{
    scope: string;
    pending: boolean;
    result: SshHostTrustInspection | null;
    error: string | null;
  }>({ scope, pending: false, result: null, error: null });
  // Never show a previous target's fingerprint, even in the commit before effects run.
  const inspection = snapshot.scope === scope
    ? snapshot
    : { pending: false, result: null, error: null };

  useEffect(() => {
    setInspection({ scope, pending: false, result: null, error: null });
    return () => {
      generation.current++;
      inFlight.current = false;
    };
  }, [scope]);

  const validEndpoint = host.trim().length > 0 && Number.isInteger(port) && port > 0 && port <= 65535;
  const inspect = async () => {
    if (!validEndpoint || inFlight.current) return;
    const current = ++generation.current;
    inFlight.current = true;
    setInspection({ scope, pending: true, result: null, error: null });
    try {
      const result = await sshInspectHostTrust(host.trim(), port, value);
      if (generation.current === current) setInspection({ scope, pending: false, result, error: null });
    } catch (error) {
      if (generation.current === current) {
        setInspection({ scope, pending: false, result: null, error: String(error) });
      }
    } finally {
      if (generation.current === current) inFlight.current = false;
    }
  };

  return (
    <div className="space-y-1.5" data-ssh-host-trust={policy}>
      <Label htmlFor={id}>{t("settings.connections.knownHostsPath")}</Label>
      <Input
        id={id}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={t("settings.connections.knownHostsPlaceholder")}
        aria-describedby={`${id}-help`}
        className="h-[34px] rounded-[14px] text-[13px]"
      />
      <p id={`${id}-help`} className="text-[11px] leading-relaxed text-text-secondary">
        {t(policy === "accept_new" ? "settings.connections.acceptNewFileHelp" : "settings.connections.knownHostsHelp")}
      </p>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={inspect}
        disabled={!validEndpoint || inspection.pending}
        aria-controls={`${id}-inspection`}
      >
        {t(inspection.pending ? "settings.connections.checking" : "settings.connections.inspectTrust")}
      </Button>
      <div id={`${id}-inspection`} aria-live="polite" aria-busy={inspection.pending}>
        {inspection.error && <p role="alert" className="break-words text-[11px] text-red-500">{inspection.error}</p>}
        {inspection.result && (
          <div role="status" className="space-y-1.5 rounded-lg border border-border-theme p-2 text-[11px]">
            <p className="break-all font-medium text-text-base">{inspection.result.host}:{inspection.result.port}</p>
            <p className="break-all text-text-secondary">{inspection.result.known_hosts_path}</p>
            <ul className="space-y-1">
              {inspection.result.keys.map((key) => (
                <li key={key.line_number} className="break-all font-mono text-text-base">
                  {key.algorithm} · {key.fingerprint} · {t("settings.connections.trustLine")} {key.line_number}
                </li>
              ))}
            </ul>
            <p className="text-text-secondary">{t("settings.connections.trustInspectionHelp")}</p>
          </div>
        )}
      </div>
      {onPolicyChange && (
        <div className="space-y-1.5 pt-3">
          <Label htmlFor={`${id}-policy`}>{t("settings.connections.hostKeyPolicy")}</Label>
          <Select value={policy} onValueChange={(next) => {
            if (next === "strict" || next === "accept_new") onPolicyChange(next);
          }}>
            <SelectTrigger id={`${id}-policy`} aria-describedby={`${id}-policy-help`}>
              <SelectValue placeholder={t(policy === "accept_new" ? "settings.connections.acceptNewPolicy" : "settings.connections.strictPolicy")} />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="strict">{t("settings.connections.strictPolicy")}</SelectItem>
              <SelectItem value="accept_new">{t("settings.connections.acceptNewPolicy")}</SelectItem>
            </SelectContent>
          </Select>
          <p id={`${id}-policy-help`} role="note" className="text-[11px] leading-relaxed text-text-secondary">
            {t(policy === "accept_new" ? "settings.connections.acceptNewHelp" : "settings.connections.strictPolicyHelp")}
          </p>
        </div>
      )}
    </div>
  );
}
