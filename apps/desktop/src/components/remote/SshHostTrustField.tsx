import { useTranslation } from "react-i18next";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";

/** Shared by both SSH editors; selecting a source is not automatic key enrollment. */
export function SshHostTrustField({ id, value, onChange }: {
  id: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="space-y-1.5" data-ssh-host-trust="strict">
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
        {t("settings.connections.knownHostsHelp")}
      </p>
    </div>
  );
}
