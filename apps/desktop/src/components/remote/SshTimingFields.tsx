import { useId } from "react";
import { useTranslation } from "react-i18next";
import type { SshConnectionTiming } from "../../api";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";

export interface SshTimingForm {
  connectTimeout: string;
  inactivityTimeout: string;
  keepaliveInterval: string;
  keepaliveMax: string;
}

export function sshTimingFromDto(timing?: SshConnectionTiming): SshTimingForm {
  return {
    connectTimeout: String(timing?.connect_timeout_secs ?? 20),
    inactivityTimeout: timing?.inactivity_timeout_secs == null ? "" : String(timing.inactivity_timeout_secs),
    keepaliveInterval: timing?.keepalive_interval_secs == null ? "" : String(timing.keepalive_interval_secs),
    keepaliveMax: String(timing?.keepalive_max ?? 3),
  };
}

export function canSaveSshTiming(value?: SshTimingForm): boolean {
  if (!value) return true;
  const seconds = (text: string, optional: boolean) => (optional && text === "")
    || (/^\d+$/.test(text) && Number(text) >= 1 && Number(text) <= 86400);
  return seconds(value.connectTimeout, false) && seconds(value.inactivityTimeout, true)
    && seconds(value.keepaliveInterval, true) && /^\d+$/.test(value.keepaliveMax)
    && Number(value.keepaliveMax) >= 1 && Number(value.keepaliveMax) <= 100;
}

export function sshTimingRequest(value?: SshTimingForm): SshConnectionTiming | undefined {
  if (!value) return undefined;
  if (!canSaveSshTiming(value)) throw new Error("Invalid SSH timing settings");
  return {
    connect_timeout_secs: Number(value.connectTimeout),
    inactivity_timeout_secs: value.inactivityTimeout === "" ? null : Number(value.inactivityTimeout),
    keepalive_interval_secs: value.keepaliveInterval === "" ? null : Number(value.keepaliveInterval),
    keepalive_max: Number(value.keepaliveMax),
  };
}

export function SshTimingFields({ value, onChange }: { value: SshTimingForm; onChange: (value: SshTimingForm) => void }) {
  const id = useId();
  const { t } = useTranslation();
  return <div className="grid grid-cols-2 gap-3" data-ssh-timing="true">
    {(["connectTimeout", "inactivityTimeout", "keepaliveInterval", "keepaliveMax"] as const).map((field) => {
      const optional = field === "inactivityTimeout" || field === "keepaliveInterval";
      return <div key={field} className="space-y-1.5">
        <Label htmlFor={`${id}-${field}`}>{t(`settings.connections.timing.${field}`)}</Label>
        <Input id={`${id}-${field}`} type="number" min={1} max={field === "keepaliveMax" ? 100 : 86400} step={1}
          required={!optional} value={value[field]} placeholder={optional ? t("settings.connections.timing.off") : undefined}
          disabled={field === "keepaliveMax" && value.keepaliveInterval === ""}
          onChange={(event) => onChange({ ...value, [field]: event.target.value })} />
      </div>;
    })}
  </div>;
}
