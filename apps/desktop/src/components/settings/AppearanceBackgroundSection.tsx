import { useEffect, useId, useRef, useState } from "react";
import { ImagePlus, LoaderCircle, Trash2, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "../shadcn/button";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import { Slider } from "../shadcn/slider";
import { useThemeContext } from "../../theme/ThemeProvider";
import {
  BACKGROUND_IMAGE_ACCEPT,
  BackgroundImageError,
  prepareBackgroundImage,
  previewBackgroundTransparency,
} from "../../theme/themeBackground";
import type { ThemePreferences } from "../../theme/themeTypes";

const KEY = "settings.appearance.wallpaper";

export function AppearanceBackgroundSection() {
  const { t } = useTranslation();
  const { state, updatePreferences } = useThemeContext();
  const { backgroundImage, backgroundTransparency } = state.preferences;
  const [transparency, setTransparency] = useState(backgroundTransparency);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const uploadRevision = useRef(0);
  const sliderId = useId();

  useEffect(() => {
    setTransparency(backgroundTransparency);
    return () => previewBackgroundTransparency(backgroundTransparency);
  }, [backgroundTransparency]);
  useEffect(() => () => { uploadRevision.current++; }, []);

  const save = (patch: Partial<ThemePreferences>): boolean => {
    try {
      updatePreferences(patch);
      setError(null);
      return true;
    } catch (failure) {
      setError(t(`${KEY}.saveFailed`, {
        reason: failure instanceof Error ? failure.message : String(failure),
      }));
      return false;
    }
  };

  const upload = async (file: File | undefined) => {
    if (!file || uploading) return;
    const revision = ++uploadRevision.current;
    setUploading(true);
    setError(null);
    try {
      const image = await prepareBackgroundImage(file);
      if (revision === uploadRevision.current) save({ backgroundImage: image });
    } catch (failure) {
      if (revision !== uploadRevision.current) return;
      setError(failure instanceof BackgroundImageError
        ? t(`${KEY}.errors.${failure.code}`)
        : t(`${KEY}.uploadFailed`, {
          reason: failure instanceof Error ? failure.message : String(failure),
        }));
    } finally {
      if (revision === uploadRevision.current) setUploading(false);
    }
  };

  return (
    <section className="mb-6 overflow-hidden rounded-xl border border-border-theme bg-elevated-bg shadow-sm"
      aria-labelledby={`${sliderId}-title`}>
      <div className="border-b border-border-theme p-4">
        <h2 id={`${sliderId}-title`} className="text-[15px] font-medium text-text-base">{t(`${KEY}.title`)}</h2>
        <p className="mt-1 text-[12px] leading-relaxed text-text-secondary">{t(`${KEY}.description`)}</p>
      </div>
      <div className="space-y-4 p-4">
        <div className="flex h-36 items-center justify-center overflow-hidden rounded-lg border border-dashed border-border-theme bg-ui-tint">
          {backgroundImage ? (
            <img src={backgroundImage.dataUrl} alt={t(`${KEY}.preview`)} className="h-full w-full object-cover" />
          ) : (
            <div className="flex flex-col items-center gap-2 text-[12px] text-text-secondary">
              <ImagePlus className="h-6 w-6" aria-hidden="true" />
              <span>{t(`${KEY}.empty`)}</span>
            </div>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Input ref={fileInput} type="file" accept={BACKGROUND_IMAGE_ACCEPT} className="hidden"
            tabIndex={-1} aria-label={t(`${KEY}.upload`)} disabled={uploading}
            onChange={event => {
              const file = event.currentTarget.files?.[0];
              event.currentTarget.value = "";
              void upload(file);
            }} />
          <Button variant="outline" size="sm" disabled={uploading} onClick={() => fileInput.current?.click()}>
            {uploading ? <LoaderCircle className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              : <Upload className="h-3.5 w-3.5" aria-hidden="true" />}
            {t(`${KEY}.${uploading ? "processing" : backgroundImage ? "replace" : "upload"}`)}
          </Button>
          {backgroundImage && <>
            <Button variant="ghost" size="sm" disabled={uploading} onClick={() => save({ backgroundImage: null })}>
              <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />{t(`${KEY}.remove`)}
            </Button>
            <span className="min-w-0 flex-1 break-all text-[12px] text-text-secondary">
              {backgroundImage.name}
            </span>
          </>}
        </div>
        <p className="text-[12px] leading-relaxed text-text-secondary">{t(`${KEY}.formats`)}</p>
        <div className="border-t border-border-theme pt-4">
          <div className="mb-3 flex items-center justify-between gap-4">
            <Label htmlFor={sliderId} className="text-[13px] text-text-base">{t(`${KEY}.transparency`)}</Label>
            <output htmlFor={sliderId} className="text-[12px] tabular-nums text-text-secondary">{transparency}%</output>
          </div>
          <Slider min={0} max={100} step={1} value={[transparency]} disabled={!backgroundImage || uploading}
            className="background-transparency-slider"
            thumbProps={{ id: sliderId, "aria-label": t(`${KEY}.transparency`), "aria-valuetext": `${transparency}%` }}
            onValueChange={([value]) => {
              setTransparency(value);
              previewBackgroundTransparency(value);
            }}
            onValueCommit={([value]) => {
              if (!save({ backgroundTransparency: value })) {
                setTransparency(backgroundTransparency);
                previewBackgroundTransparency(backgroundTransparency);
              }
            }} />
          <p className="mt-3 text-[12px] leading-relaxed text-text-secondary">{t(`${KEY}.transparencyHint`)}</p>
        </div>
        {error && <p role="alert" className="text-[12px] leading-relaxed text-red-500">{error}</p>}
      </div>
    </section>
  );
}
