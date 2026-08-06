import { useEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { useThemeContext } from "../../theme/ThemeProvider";
import type { ThemePreset, ThemeVariant } from "../../theme/themeTypes";
import { Panel } from "../ui/Panel";
import { ListItem } from "../ui/ListItem";

function PaletteSwatches({ preset, variant }: { preset: ThemePreset; variant: ThemeVariant }) {
  const palette = preset.variants[variant];
  if (!palette) return null;
  return (
    <div className="flex items-center mr-2">
      <div
        className="w-3.5 h-3.5 rounded-full border border-black/10"
        style={{ backgroundColor: palette.background }}
      />
      <div
        className="w-3.5 h-3.5 rounded-full border border-black/10 -ml-1"
        style={{ backgroundColor: palette.foreground }}
      />
      <div
        className="w-3.5 h-3.5 rounded-full border border-black/10 -ml-1"
        style={{ backgroundColor: palette.accent }}
      />
    </div>
  );
}

export function ThemePresetMenu({ variant }: { variant: ThemeVariant }) {
  const { t } = useTranslation();
  const {
    state,
    availablePresets,
    selectPreset,
    renameCustomPreset,
    deleteCustomPreset,
  } = useThemeContext();
  const [isOpen, setIsOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const selectedId = state.selectedPresetIds[variant];
  const isCustom = selectedId.startsWith("custom-") &&
    !state.customPresets.some((p) => p.id === selectedId);

  const presets = availablePresets.filter((p) => p.variants[variant]);
  const selected = presets.find((p) => p.id === selectedId);
  const displayName = isCustom
    ? t("settings.appearance.customTheme", "自定义")
    : selected?.name ?? selectedId;

  useEffect(() => {
    if (!isOpen) return;
    const onPointerDown = (e: PointerEvent) => {
      if (!containerRef.current?.contains(e.target as Node)) {
        setIsOpen(false);
      }
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setIsOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [isOpen]);

  const sourceLabel = (preset: ThemePreset) => {
    if (preset.source === "custom") return t("settings.appearance.sourceCustom", "自定义");
    if (preset.source === "imported") return t("settings.appearance.sourceImported", "导入");
    return null;
  };

  const currentPalette = state.workingPalettes[variant];

  return (
    <div className="relative" ref={containerRef}>
      <button
        type="button"
        className="flex items-center bg-black/5 hover:bg-black/5 rounded-lg px-2 py-1 cursor-pointer transition-colors min-w-[130px] justify-between"
        onClick={() => setIsOpen((v) => !v)}
      >
        <div className="flex items-center">
          <div className="flex items-center mr-2">
            <div
              className="w-3.5 h-3.5 rounded-full border border-black/10"
              style={{ backgroundColor: currentPalette.background }}
            />
            <div
              className="w-3.5 h-3.5 rounded-full border border-black/10 -ml-1"
              style={{ backgroundColor: currentPalette.foreground }}
            />
            <div
              className="w-3.5 h-3.5 rounded-full border border-black/10 -ml-1"
              style={{ backgroundColor: currentPalette.accent }}
            />
          </div>
          <span className="text-[12px] font-medium text-text-base mr-3">{displayName}</span>
        </div>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
      </button>

      {isOpen && (
        <Panel className="absolute top-full right-0 mt-1 z-20 py-1 w-[220px] max-h-[280px] overflow-y-auto">
          {presets.map((preset) => {
            const label = sourceLabel(preset);
            const isBuiltin = preset.source === "builtin";
            return (
              <ListItem
                key={preset.id}
                className="px-3 py-1.5 cursor-pointer group"
                onClick={() => {
                  selectPreset(variant, preset.id);
                  setIsOpen(false);
                }}
              >
                <div className="flex items-center min-w-0">
                  <PaletteSwatches preset={preset} variant={variant} />
                  <span className="text-[13px] text-text-base truncate">{preset.name}</span>
                  {label && (
                    <span className="ml-2 text-[10px] text-text-secondary shrink-0">{label}</span>
                  )}
                </div>
                <div className="flex items-center shrink-0">
                  {!isBuiltin && (
                    <>
                      <button
                        type="button"
                        title={t("settings.appearance.rename", "重命名")}
                        className="opacity-0 group-hover:opacity-100 text-[10px] text-text-secondary hover:text-text-base px-1"
                        onClick={(e) => {
                          e.stopPropagation();
                          const name = window.prompt(
                            t("settings.appearance.renamePrompt", "新名称"),
                            preset.name,
                          );
                          if (name && name.trim()) renameCustomPreset(preset.id, name.trim());
                        }}
                      >
                        <FontAwesomeIcon icon={["fas", "pen"]} />
                      </button>
                      <button
                        type="button"
                        title={t("settings.appearance.delete", "删除")}
                        className="opacity-0 group-hover:opacity-100 text-[10px] text-text-secondary hover:text-red-500 px-1"
                        onClick={(e) => {
                          e.stopPropagation();
                          if (
                            window.confirm(
                              t("settings.appearance.deleteConfirm", "删除该自定义方案？"),
                            )
                          ) {
                            deleteCustomPreset(preset.id);
                          }
                        }}
                      >
                        <FontAwesomeIcon icon={["fas", "trash"]} />
                      </button>
                    </>
                  )}
                  <div className="w-4 flex justify-end">
                    {selectedId === preset.id && (
                      <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />
                    )}
                  </div>
                </div>
              </ListItem>
            );
          })}
        </Panel>
      )}
    </div>
  );
}
