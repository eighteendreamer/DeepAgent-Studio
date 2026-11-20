import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { MessageCircle, Moon, RotateCcw, Sparkles } from "lucide-react";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../shadcn/select";
import { getPet, listenPet, nativePet, petAsset, resetPet, updatePet, type PetSnapshot, type Preferences } from "../../pet/api";

export function PetSettings() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<PetSnapshot>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [action, setAction] = useState("");
  const [load, setLoad] = useState(0);

  useEffect(() => {
    if (!nativePet) return;
    let active = true;
    const cleanup = listenPet<Preferences>("preferences", preferences => {
      if (active) setSnapshot(current => current && { ...current, preferences });
    });
    void cleanup.then(() => getPet()).then(value => {
      if (active) setSnapshot(value);
    }).catch(reason => { if (active) setError(String(reason)); });
    return () => { active = false; void cleanup.then(unlisten => unlisten(), () => undefined); };
  }, [load]);

  const run = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try { await task(); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };
  const change = (patch: Partial<Preferences>) => void run(async () => {
    const preferences = await updatePet(patch);
    setSnapshot(current => current && { ...current, preferences });
  });
  const prefs = snapshot?.preferences;
  const pet = snapshot?.catalog.characters.find(item => item.id === prefs?.characterId);
  const awake = Boolean(prefs?.enabled && !prefs.sleeping);
  const preview = pet?.actions[action];
  const selectedSize = String(prefs?.size ?? 462);
  const sizes = Array.from(new Set([330, pet?.defaultSize ?? 462, 620, Number(selectedSize)])).sort((a, b) => a - b);

  return (
    <section className="max-w-[700px] text-text-base">
      <h1 className="mb-2 text-2xl font-semibold">{t("settings.tabs.pet")}</h1>
      <p className="mb-6 text-[13px] text-text-secondary">{t("desktopPet.description")}</p>
      {!nativePet && <p className="text-sm text-text-secondary">{t("desktopPet.desktopOnly")}</p>}
      {nativePet && !snapshot && !error && <p role="status" className="text-sm text-text-secondary">{t("desktopPet.loading")}</p>}
      {error && <div role="alert" className="mb-4 rounded-lg border border-border-theme p-3 text-sm">
        <p className="text-red-500">{error}</p>
        {!snapshot && <Button size="sm" variant="ghost" className="mt-2" onClick={() => { setError(""); setLoad(n => n + 1); }}>{t("desktopPet.retry")}</Button>}
      </div>}
      {snapshot && prefs && pet && <>
        <div className="overflow-hidden rounded-xl border border-border-theme bg-elevated-bg">
          <div className="grid gap-4 p-4 sm:grid-cols-[200px_minmax(0,1fr)]">
            <div className="flex h-36 items-center justify-center overflow-hidden rounded-lg border border-border-theme bg-sidebar-bg">
              {preview ? <video key={preview.file} src={petAsset(snapshot.resourceRoot, preview.file)} autoPlay muted playsInline
                className="h-full w-full object-contain" onEnded={() => setAction("")} onError={() => { setAction(""); setError(t("desktopPet.previewFailed")); }} /> :
                <img src={petAsset(snapshot.resourceRoot, pet.thumbnail)} alt={pet.name} className="h-full w-full object-contain" />}
            </div>
            <div className="flex min-w-0 flex-col justify-center gap-3">
              <div><div className="text-sm font-semibold">{pet.name}</div><div className="mt-1 text-xs text-text-secondary">{t("desktopPet.characterSource", { count: Object.keys(pet.actions).length })}</div></div>
              {snapshot.catalog.characters.length > 1 && <Select value={pet.id} onValueChange={characterId => { setAction(""); change({ characterId }); }}>
                <SelectTrigger disabled={busy} aria-label={t("desktopPet.character")}><SelectValue placeholder={pet.name} /></SelectTrigger>
                <SelectContent>{snapshot.catalog.characters.map(item => <SelectItem key={item.id} value={item.id}>{item.name}</SelectItem>)}</SelectContent>
              </Select>}
              <Select value={action} onValueChange={setAction}>
                <SelectTrigger className="w-full max-w-56" aria-label={t("desktopPet.preview")}><SelectValue placeholder={action || t("desktopPet.preview")} /></SelectTrigger>
                <SelectContent className="max-h-[min(18rem,var(--radix-dropdown-menu-content-available-height))] overflow-y-auto">{Object.keys(pet.actions).map(name => <SelectItem key={name} value={name}>{name}</SelectItem>)}</SelectContent>
              </Select>
            </div>
          </div>
          <div className="flex flex-wrap gap-2 border-t border-border-theme p-4">
            <Button variant="outline" size="sm" disabled={busy} onClick={() => change(awake ? { sleeping: true } : { enabled: true, sleeping: false })}>
              {awake ? <Moon size={14} /> : <Sparkles size={14} />}{t(awake ? "desktopPet.sleep" : "desktopPet.wake")}
            </Button>
            <Button variant="ghost" size="sm" disabled={busy} onClick={() => void run(resetPet)}><RotateCcw size={14} />{t("desktopPet.reset")}</Button>
            <Button variant="ghost" size="sm" disabled><MessageCircle size={14} />{t("desktopPet.chat")}</Button>
          </div>
        </div>
        <div className="mt-4 divide-y divide-border-theme rounded-xl border border-border-theme bg-elevated-bg">
          {(["enabled", "alwaysOnTop", "followWork", "bubbles"] as const).map(key => (
            <label key={key} className="flex items-center justify-between gap-3 px-4 py-3 text-[13px]">
              <span>{t(`desktopPet.${key}`)}</span>
              <Checkbox checked={prefs[key]} disabled={busy} onCheckedChange={checked => change({ [key]: checked === true })} />
            </label>
          ))}
          <div className="flex items-center justify-between gap-3 px-4 py-3 text-[13px]">
            <span>{t("desktopPet.size")}</span>
            <Select value={selectedSize} onValueChange={value => change({ size: Number(value) })}>
              <SelectTrigger disabled={busy} aria-label={t("desktopPet.size")} className="w-28"><SelectValue placeholder={`${selectedSize} px`} /></SelectTrigger>
              <SelectContent>{sizes.map(size => <SelectItem key={size} value={String(size)}>{size} px</SelectItem>)}</SelectContent>
            </Select>
          </div>
        </div>
      </>}
    </section>
  );
}
