import { useEffect, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { Check, MessageCircle, RotateCcw } from "lucide-react";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from "../shadcn/popover";
import { cn } from "../shadcn/utils";
import { getPet, isPetAwake, listenPet, nativePet, petAsset, resetPet, resolveAction, updatePet,
  type Character, type PetSnapshot, type Preferences } from "../../pet/api";

function PetPreview({ pet, root }: { pet: Character; root: string }) {
  const [failed, setFailed] = useState(false);
  const idle = resolveAction(pet, "idle");
  const thumbnail = petAsset(root, pet.thumbnail);
  return idle && !failed
    ? <video src={petAsset(root, idle.file)} poster={thumbnail} aria-label={pet.name}
        autoPlay muted loop playsInline onError={() => setFailed(true)} className="h-40 w-full object-contain" />
    : <img src={thumbnail} alt={pet.name} className="h-40 w-full object-contain" />;
}

export function PetSettings() {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<PetSnapshot>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [customize, setCustomize] = useState(false);
  const [size, setSize] = useState("");
  const [load, setLoad] = useState(0);

  useEffect(() => {
    if (!nativePet) return;
    let active = true;
    let latest: Preferences | undefined;
    const listener = listenPet<Preferences>("preferences", preferences => {
      latest = preferences;
      if (active) setSnapshot(current => current && { ...current, preferences });
    });
    void listener.then(getPet).then(value => {
      if (active) setSnapshot(latest ? { ...value, preferences: latest } : value);
    }).catch(reason => { if (active) setError(String(reason)); });
    return () => { active = false; void listener.then(unlisten => unlisten(), () => undefined); };
  }, [load]);

  const run = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try { await task(); return true; }
    catch (reason) { setError(String(reason)); return false; }
    finally { setBusy(false); }
  };
  const change = (patch: Partial<Preferences>) => run(async () => {
    const preferences = await updatePet(patch);
    setSnapshot(current => current && { ...current, preferences });
  });
  const prefs = snapshot?.preferences;
  const pet = snapshot?.catalog.characters.find(item => item.id === prefs?.characterId);
  const awake = isPetAwake(prefs);
  const pixels = Number(size);
  const validSize = snapshot && Number.isInteger(pixels) && pixels >= snapshot.sizeMin && pixels <= snapshot.sizeMax;
  const saveSize = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!validSize || busy) return;
    if (await change({ size: pixels })) setCustomize(false);
  };

  return (
    <section className="max-w-[700px] text-text-base">
      <div className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h1 className="mb-2 text-2xl font-semibold">{t("settings.tabs.pet")}</h1>
          <p className="text-[13px] text-text-secondary">{t("desktopPet.description")}</p>
        </div>
        {prefs && <Button variant="outline" size="sm" className="mt-0.5 bg-elevated-bg hover:bg-ui-tint"
          disabled={busy} onClick={() => void change({ enabled: !awake })}>
          {t(awake ? "desktopPet.turnOff" : "desktopPet.turnOn")}
        </Button>}
      </div>
      {!nativePet && <p className="text-sm text-text-secondary">{t("desktopPet.desktopOnly")}</p>}
      {nativePet && !snapshot && !error && <p role="status" className="text-sm text-text-secondary">{t("desktopPet.loading")}</p>}
      {error && !customize && <div role="alert" className="mb-4 rounded-lg border border-border-theme p-3 text-sm">
        <p className="text-red-500">{error}</p>
        {!snapshot && <Button size="sm" variant="ghost" className="mt-2" onClick={() => { setError(""); setLoad(n => n + 1); }}>{t("desktopPet.retry")}</Button>}
      </div>}
      {snapshot && prefs && pet && <>
        <div className="rounded-xl border border-border-theme bg-elevated-bg px-4 pb-3 pt-4">
          <div className="mx-auto flex min-h-48 max-w-sm flex-col items-center justify-center gap-1 text-center">
            <PetPreview key={pet.id} pet={pet} root={snapshot.resourceRoot} />
            <div className="text-sm font-semibold">{pet.name}</div>
          </div>
          <div className="flex justify-end">
            <Popover morph={false} open={customize} onOpenChange={open => {
              if (open) { setSize(String(prefs.size)); setError(""); }
              setCustomize(open);
            }}>
              <PopoverTrigger render={<Button variant="outline" size="sm" className="whitespace-nowrap bg-elevated-bg hover:bg-ui-tint" />}>
                {t("desktopPet.customize")}
              </PopoverTrigger>
              <PopoverContent align="end" className="max-h-[var(--available-height)] w-56 max-w-[calc(100vw-2rem)] overflow-y-auto p-3">
                <PopoverTitle className="mb-3 text-sm font-semibold">{t("desktopPet.customize")}</PopoverTitle>
                <form onSubmit={event => void saveSize(event)}>
                  <label className="block text-[13px]">
                    {t("desktopPet.size")}
                    <div className="mt-1.5 flex items-center gap-2">
                      <Input type="number" min={snapshot.sizeMin} max={snapshot.sizeMax} step={1} value={size}
                        disabled={busy} onChange={event => setSize(event.target.value)} className="h-8 bg-elevated-bg" />
                      <span className="text-text-secondary" aria-hidden="true">px</span>
                    </div>
                  </label>
                  <div className="mt-1.5 text-xs text-text-secondary">{snapshot.sizeMin}–{snapshot.sizeMax} px</div>
                  <div className="mt-3 flex justify-end">
                    <Button type="submit" variant="outline" size="sm" className="bg-elevated-bg hover:bg-ui-tint"
                      disabled={!validSize || busy}>{t("desktopPet.apply")}</Button>
                  </div>
                </form>
                <div className="mt-3 space-y-2 border-t border-border-theme pt-3">
                  {(["alwaysOnTop", "followWork", "bubbles"] as const).map(key => (
                    <label key={key} className="flex items-center justify-between gap-3 text-[13px]">
                      <span>{t(`desktopPet.${key}`)}</span>
                      <Checkbox checked={prefs[key]} disabled={busy} onCheckedChange={checked => void change({ [key]: checked === true })} />
                    </label>
                  ))}
                </div>
                <div className="mt-3 flex flex-wrap gap-1 border-t border-border-theme pt-2">
                  <Button variant="ghost" size="sm" disabled={busy} onClick={() => void run(resetPet)}><RotateCcw size={14} />{t("desktopPet.reset")}</Button>
                  <Button variant="ghost" size="sm" disabled><MessageCircle size={14} />{t("desktopPet.chat")}</Button>
                </div>
                {error && <p role="alert" className="mt-2 text-xs text-red-500">{error}</p>}
              </PopoverContent>
            </Popover>
          </div>
        </div>
        <h2 className="mb-3 mt-6 text-sm font-semibold">{t("desktopPet.collection")}</h2>
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
          {snapshot.catalog.characters.map(character => {
            const selected = character.id === pet.id;
            return <Button key={character.id} variant="outline" aria-pressed={selected} disabled={busy}
              onClick={() => { if (!selected) void change({ characterId: character.id }); }}
              className={cn("relative h-auto min-h-36 flex-col gap-2 rounded-xl bg-elevated-bg p-3 shadow-none hover:bg-ui-tint",
                selected && "ring-1 ring-border-theme")}>
              {selected && <Check size={14} className="absolute right-3 top-3 text-text-secondary" aria-hidden="true" />}
              <img src={petAsset(snapshot.resourceRoot, character.thumbnail)} alt="" className="h-20 w-full object-contain" />
              <span className="text-center">{character.name}</span>
            </Button>;
          })}
        </div>
      </>}
    </section>
  );
}
