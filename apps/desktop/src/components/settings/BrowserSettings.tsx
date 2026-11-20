import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { getBrowserSettings, setBrowserSettings, type WorkspaceBrowserSettings } from "../../api";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { ToggleSwitch } from "../ui/ToggleSwitch";

function SettingsRow({ label, children }: { label: string; children: ReactNode }) {
  return <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
    <span className="text-sm font-medium text-text-base">{label}</span>
    {children}
  </div>;
}

function DomainList({ title, description, domains, busy, save }: {
  title: string;
  description: string;
  domains: string[];
  busy: boolean;
  save: (domains: string[]) => Promise<boolean>;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  return <section className="rounded-xl border border-border-theme p-4">
    <h2 className="text-sm font-medium text-text-base">{title}</h2>
    <p className="mt-1 text-sm text-text-secondary">{description}</p>
    <form className="mt-3 flex items-center gap-2" onSubmit={async event => {
      event.preventDefault();
      const domain = draft.trim();
      if (domain && await save([...domains, domain])) setDraft("");
    }}>
      <Input aria-label={title} placeholder="example.com" value={draft} disabled={busy}
        onChange={event => setDraft(event.target.value)} className="min-w-0 flex-1" />
      <Button variant="secondary" type="submit" disabled={busy || !draft.trim()}>{t("settings.browser.add")}</Button>
    </form>
    {domains.length > 0 && <ul className="mt-3 divide-y divide-border-theme">
      {domains.map(domain => <li key={domain} className="flex items-center justify-between gap-2 py-1.5 text-sm">
        <span className="min-w-0 break-all">{domain}</span>
        <Button type="button" size="sm" variant="ghost" disabled={busy}
          aria-label={`${t("settings.browser.remove")} ${domain}`}
          onClick={() => void save(domains.filter(value => value !== domain))}>{t("settings.browser.remove")}</Button>
      </li>)}
    </ul>}
  </section>;
}

export function BrowserSettings() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<WorkspaceBrowserSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    void getBrowserSettings().then(value => { if (active) setSettings(value); })
      .catch(cause => { if (active) setError(String(cause)); });
    return () => { active = false; };
  }, []);

  async function save(patch: Partial<WorkspaceBrowserSettings>): Promise<boolean> {
    if (!settings || busy) return false;
    setBusy(true);
    setError("");
    try {
      setSettings(await setBrowserSettings({ ...settings, ...patch }));
      return true;
    } catch (cause) {
      setError(String(cause));
      return false;
    } finally {
      setBusy(false);
    }
  }

  return <div className="max-w-[700px] space-y-5 pb-20">
    <h1 className="text-2xl font-semibold text-text-base">{t("settings.browser.title")}</h1>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {settings && <>
      <div className="divide-y divide-border-theme rounded-xl border border-border-theme">
        <SettingsRow label={t("settings.browser.enabled")}>
          <ToggleSwitch aria-label={t("settings.browser.enabled")} checked={settings.enabled} disabled={busy}
            onChange={() => void save({ enabled: !settings.enabled })} />
        </SettingsRow>
        <SettingsRow label={t("settings.browser.screenshots")}>
          <ToggleSwitch aria-label={t("settings.browser.screenshots")} checked={settings.screenshots_enabled} disabled={busy}
            onChange={() => void save({ screenshots_enabled: !settings.screenshots_enabled })} />
        </SettingsRow>
        <SettingsRow label={t("settings.browser.approval")}>
          <Select value={settings.require_approval ? "ask" : "inherit"}
            onValueChange={value => void save({ require_approval: value === "ask" })}>
            <SelectTrigger className="w-48" disabled={busy} aria-label={t("settings.browser.approval")}>
              <SelectValue placeholder={t(settings.require_approval ? "settings.browser.alwaysAsk" : "settings.browser.inherit")} />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="inherit">{t("settings.browser.inherit")}</SelectItem>
              <SelectItem value="ask">{t("settings.browser.alwaysAsk")}</SelectItem>
            </SelectContent>
          </Select>
        </SettingsRow>
      </div>
      <DomainList title={t("settings.browser.blockedDomains")} description={t("settings.browser.blockedDesc")}
        domains={settings.blocked_domains} busy={busy} save={blocked_domains => save({ blocked_domains })} />
      <DomainList title={t("settings.browser.allowedDomains")} description={t("settings.browser.allowedDesc")}
        domains={settings.allowed_domains} busy={busy} save={allowed_domains => save({ allowed_domains })} />
    </>}
  </div>;
}
