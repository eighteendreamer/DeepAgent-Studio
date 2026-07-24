import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTheme, type ThemeMode, type ThemeSwitchOrigin } from "../../hooks/useTheme";

function ToggleSwitch({ checked, onChange }: { checked: boolean; onChange: () => void }) {
  return (
    <div 
      className={`w-9 h-5 rounded-full relative cursor-pointer transition-colors ${checked ? 'bg-blue-500' : 'bg-gray-300'}`}
      onClick={onChange}
    >
      <div className={`w-3.5 h-3.5 rounded-full bg-white absolute top-[3px] transition-transform ${checked ? 'translate-x-[20px]' : 'translate-x-[3px]'}`} />
    </div>
  );
}

function SegmentedControl({
  options,
  value,
  onChange,
  indicatorLayoutId,
}: {
  options: { label: React.ReactNode, value: string }[];
  value: string;
  onChange: (val: string, origin: ThemeSwitchOrigin) => void;
  indicatorLayoutId?: string;
}) {
  return (
    <div className="relative flex items-center bg-gray-100 p-0.5 rounded-lg border border-border-theme">
      {options.map((opt) => {
        const selected = value === opt.value;
        return (
          <button
            type="button"
            key={opt.value}
            onClick={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              onChange(opt.value, {
                x: rect.left + rect.width / 2,
                y: rect.top + rect.height / 2,
              });
            }}
            className={`relative flex items-center justify-center px-3 py-1 text-[12px] font-medium transition-colors rounded-md ${selected ? `text-text-base ${indicatorLayoutId ? '' : 'bg-white shadow-[0_1px_2px_rgb(0,0,0,0.1)]'}` : 'text-text-secondary hover:text-text-base'}`}
          >
            {selected && indicatorLayoutId && (
              <span
                className="absolute inset-0 z-0 rounded-md bg-white shadow-[0_1px_2px_rgb(0,0,0,0.1)]"
              />
            )}
            <span className="relative z-10 flex items-center">{opt.label}</span>
          </button>
        );
      })}
    </div>
  );
}

function PetItem({ name, desc, icon, iconColor, selected }: { name: string, desc: string, icon: IconProp, iconColor: string, selected: boolean }) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center justify-between p-3 border border-border-theme rounded-xl hover:border-gray-300 transition-colors cursor-pointer">
      <div className="flex items-center">
        <div className="w-12 h-12 rounded-lg bg-gray-50 flex items-center justify-center mr-4 border border-border-theme shadow-sm">
          <FontAwesomeIcon icon={icon} className={`text-xl ${iconColor}`} />
        </div>
        <div>
          <div className="text-[13px] font-medium text-text-base mb-0.5">{name}</div>
          <div className="text-[12px] text-text-secondary">{desc}</div>
        </div>
      </div>
      <div>
        {selected ? (
          <div className="px-4 py-1.5 bg-gray-100 text-gray-400 rounded-md text-[12px] font-medium">
            {t("settings.appearance.selected")}
          </div>
        ) : (
          <div className="px-4 py-1.5 bg-white border border-border-theme text-text-base hover:bg-gray-50 rounded-md text-[12px] font-medium transition-colors">
            {t("settings.appearance.select")}
          </div>
        )}
      </div>
    </div>
  );
}

function ThemeDropdown({ 
  selectedTheme, 
  onChange, 
  isDark 
}: { 
  selectedTheme: string, 
  onChange: (t: string) => void,
  isDark?: boolean
}) {
  const [isOpen, setIsOpen] = useState(false);
  const themes = [
    "Absolutely", "Ayu", "Catppuccin", "Codex", "Dracula", 
    "Everforest", "GitHub", "Gruvbox", "Linear", "Notion", "One"
  ];

  return (
    <div className="relative">
      <div 
        className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-2 py-1 cursor-pointer transition-colors min-w-[120px] justify-between"
        onClick={() => setIsOpen(!isOpen)}
        onBlur={() => setTimeout(() => setIsOpen(false), 200)}
        tabIndex={0}
      >
        <div className="flex items-center">
          <div className={`w-5 h-5 rounded-md ${isDark ? 'bg-gray-800 text-white' : 'bg-blue-100 text-blue-600'} font-serif text-[10px] flex items-center justify-center mr-2 font-bold`}>Aa</div>
          <span className="text-[12px] font-medium text-text-base mr-3">{selectedTheme}</span>
        </div>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
      </div>

      {isOpen && (
        <div className="absolute top-full right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-lg z-20 py-1 w-[200px] max-h-[260px] overflow-y-auto">
          {themes.map((theme) => (
            <div 
              key={theme}
              className="px-3 py-1.5 hover:bg-gray-50 cursor-pointer flex items-center justify-between"
              onMouseDown={(e) => {
                e.preventDefault();
                onChange(theme);
                setIsOpen(false);
              }}
            >
              <div className="flex items-center">
                <div className={`w-5 h-5 rounded-full ${isDark ? 'bg-gray-800 text-white' : 'bg-blue-100 text-blue-600'} font-serif text-[10px] flex items-center justify-center mr-3 font-bold`}>Aa</div>
                <span className="text-[13px] text-text-base">{theme}</span>
              </div>
              <div className="w-4 flex justify-end">
                {selectedTheme === theme && (
                  <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
export function AppearanceSettings() {
  const { t } = useTranslation();
  const { config, activeIsDark, updateThemeDetails, switchTheme } = useTheme();

  const [reduceMotion, setReduceMotion] = useState("system");
  const [diffMarker, setDiffMarker] = useState("color");
  const [pointerCursor, setPointerCursor] = useState(false);
  const [lightThemeName, setLightThemeName] = useState("Codex");
  const [darkThemeName, setDarkThemeName] = useState("Codex");

  // Determine which config to show in the editor below (always show the one matching the active toggle, or based on system if system is selected)
  const isEditingDark = activeIsDark;
  const activeDetails = isEditingDark ? config.dark : config.light;
  const activeThemeName = isEditingDark ? darkThemeName : lightThemeName;
  const setActiveThemeName = isEditingDark ? setDarkThemeName : setLightThemeName;

  const [isPetExpanded, setIsPetExpanded] = useState(true);

  return (
    <>
      <h1 className="text-2xl font-semibold text-text-base mb-10">{t("settings.appearance.title")}</h1>

      {/* Section: 主题 (The massive card) */}
      <div className="mb-6 max-w-[700px]">
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          
          {/* Header & Preview */}
          <div className="p-4 border-b border-border-theme">
            <div className="flex items-start justify-between mb-4">
              <div>
                <div className="text-[15px] font-medium text-text-base mb-1">{t("settings.appearance.theme")}</div>
                <div className="text-[12px] text-text-secondary">{t("settings.appearance.themeDesc")}</div>
              </div>
              <SegmentedControl 
                options={[
                  {label: <><FontAwesomeIcon icon={["far", "sun"]} className="mr-1.5"/>{t("settings.appearance.light")}</>, value: 'light'},
                  {label: <><FontAwesomeIcon icon={["fas", "circle-user"]} className="mr-1.5"/>{t("settings.appearance.dark")}</>, value: 'dark'}, // using circle-user as a placeholder for moon if moon not available
                  {label: <><FontAwesomeIcon icon={["fas", "desktop"]} className="mr-1.5"/>{t("settings.appearance.system")}</>, value: 'system'}
                ]} 
                value={config.mode} 
                onChange={(val, origin) => switchTheme(val as ThemeMode, origin)}
                indicatorLayoutId="appearance-theme-mode"
              />
            </div>

            {/* Theme Preview Box */}
            <div className="flex border border-border-theme rounded-lg overflow-hidden text-[11px] font-mono shadow-sm">
              <div className="flex-1 bg-white border-r border-border-theme" style={{ backgroundColor: activeDetails.bg }}>
                <div className="flex">
                  <div className="w-8 text-right pr-2 text-gray-400 select-none py-2 border-r border-border-theme" style={{ backgroundColor: `color-mix(in srgb, ${activeDetails.bg} 95%, black)` }}>
                    1<br/>2<br/>3<br/>4<br/>5
                  </div>
                  <div className="flex-1 py-2 relative" style={{ color: activeDetails.fg }}>
                    <div className="pl-3"><span className="text-purple-500">const</span> <span className="text-blue-500">themePreview</span>: <span className="text-green-500">ThemeConfig</span> = {'{'}</div>
                    <div className="pl-3 border-l-2" style={{ backgroundColor: `color-mix(in srgb, ${activeDetails.accent} 10%, transparent)`, borderColor: activeDetails.accent, color: activeDetails.accent }}>  surface: <span>"sidebar"</span>,</div>
                    <div className="pl-3 border-l-2" style={{ backgroundColor: `color-mix(in srgb, ${activeDetails.accent} 10%, transparent)`, borderColor: activeDetails.accent, color: activeDetails.accent }}>  accent: <span>"{activeDetails.accent}"</span>,</div>
                    <div className="pl-3 border-l-2" style={{ backgroundColor: `color-mix(in srgb, ${activeDetails.accent} 10%, transparent)`, borderColor: activeDetails.accent, color: activeDetails.accent }}>  contrast: <span className="text-orange-500">{activeDetails.contrast}</span>,</div>
                    <div className="pl-3">{'}'};</div>
                  </div>
                </div>
              </div>
            </div>
            
            <div className="flex justify-between items-center p-1 bg-gray-50 border-t border-border-theme">
              <button className="text-gray-400 hover:text-text-base px-2"><FontAwesomeIcon icon={["fas", "caret-left"]} /></button>
              <div className="flex space-x-1">
                <div className="w-1.5 h-1.5 rounded-full bg-gray-300"></div>
                <div className="w-1.5 h-1.5 rounded-full bg-blue-500"></div>
              </div>
              <button className="text-gray-400 hover:text-text-base px-2"><FontAwesomeIcon icon={["fas", "caret-right"]} /></button>
            </div>
          </div>

          <div className={`p-4 border-t border-border-theme space-y-6 ${isEditingDark ? 'bg-sidebar-bg' : 'bg-gray-50/50'}`}>
            {/* 浅色/深色主题 Config Nested Card */}
            <div className="border border-border-theme rounded-xl bg-white shadow-sm overflow-hidden">
              <div className={`flex items-center justify-between p-3 border-b border-border-theme ${isEditingDark ? 'bg-sidebar-bg' : 'bg-gray-50/80'}`}>
            <div className="text-[14px] font-medium text-text-base">{isEditingDark ? t("settings.appearance.darkTheme") : t("settings.appearance.lightTheme")}</div>
            <div className="flex items-center space-x-3">
              <button className="text-[12px] text-text-secondary hover:text-text-base">{t("settings.appearance.import")}</button>
              <button className="text-[12px] text-text-secondary hover:text-text-base">{t("settings.appearance.copyTheme")}</button>
              <ThemeDropdown selectedTheme={activeThemeName} onChange={setActiveThemeName} isDark={isEditingDark} />
            </div>
          </div>

          <div className="flex items-center justify-between px-4 py-3 border-b border-border-theme">
            <div className="text-[13px] text-text-base">{t("settings.appearance.accentColor")}</div>
            <div className="flex items-center justify-center rounded-lg px-2 py-1 bg-white border border-border-theme text-text-base text-[12px] font-mono shadow-sm relative">
              <input type="color" value={activeDetails.accent} onChange={(e) => updateThemeDetails(isEditingDark, { accent: e.target.value })} className="w-5 h-5 absolute left-2 top-1/2 -mt-2.5 opacity-0 cursor-pointer" />
              <div className="w-3 h-3 rounded-full border border-gray-300 mr-2" style={{ backgroundColor: activeDetails.accent }}></div>
              <input type="text" value={activeDetails.accent.toUpperCase()} onChange={(e) => updateThemeDetails(isEditingDark, { accent: e.target.value })} className="w-16 focus:outline-none bg-transparent" />
            </div>
          </div>
          <div className="flex items-center justify-between px-4 py-3 border-b border-border-theme">
            <div className="text-[13px] text-text-base">{t("settings.appearance.background")}</div>
            <div className="flex items-center justify-center rounded-lg px-2 py-1 bg-white border border-border-theme text-text-base text-[12px] font-mono shadow-sm relative">
              <input type="color" value={activeDetails.bg} onChange={(e) => updateThemeDetails(isEditingDark, { bg: e.target.value })} className="w-5 h-5 absolute left-2 top-1/2 -mt-2.5 opacity-0 cursor-pointer" />
              <div className="w-3 h-3 rounded-full border border-gray-300 mr-2" style={{ backgroundColor: activeDetails.bg }}></div>
              <input type="text" value={activeDetails.bg.toUpperCase()} onChange={(e) => updateThemeDetails(isEditingDark, { bg: e.target.value })} className="w-16 focus:outline-none bg-transparent" />
            </div>
          </div>
          <div className="flex items-center justify-between px-4 py-3 border-b border-border-theme">
            <div className="text-[13px] text-text-base">{t("settings.appearance.foreground")}</div>
            <div className="flex items-center justify-center rounded-lg px-2 py-1 bg-white border border-border-theme text-text-base text-[12px] font-mono shadow-sm relative">
              <input type="color" value={activeDetails.fg} onChange={(e) => updateThemeDetails(isEditingDark, { fg: e.target.value })} className="w-5 h-5 absolute left-2 top-1/2 -mt-2.5 opacity-0 cursor-pointer" />
              <div className="w-3 h-3 rounded-full border border-gray-300 mr-2" style={{ backgroundColor: activeDetails.fg }}></div>
              <input type="text" value={activeDetails.fg.toUpperCase()} onChange={(e) => updateThemeDetails(isEditingDark, { fg: e.target.value })} className="w-16 focus:outline-none bg-transparent" />
            </div>
          </div>
          <div className="flex items-center justify-between px-4 py-3 border-b border-border-theme">
            <div className="text-[13px] text-text-base">{t("settings.appearance.uiFont")}</div>
            <input type="text" value={activeDetails.uiFont} onChange={(e) => updateThemeDetails(isEditingDark, { uiFont: e.target.value })} className="px-3 py-1.5 bg-white border border-border-theme rounded-md text-[12px] text-text-base w-[200px] text-right truncate cursor-text shadow-sm focus:outline-none" />
          </div>
          <div className="flex items-center justify-between px-4 py-3 border-b border-border-theme">
            <div className="text-[13px] text-text-base">{t("settings.appearance.codeFont")}</div>
            <input type="text" value={activeDetails.codeFont} onChange={(e) => updateThemeDetails(isEditingDark, { codeFont: e.target.value })} className="px-3 py-1.5 bg-white border border-border-theme rounded-md text-[12px] text-text-base w-[200px] text-right truncate cursor-text shadow-sm focus:outline-none" />
          </div>
          <div className="flex items-center justify-between px-4 py-3 border-b border-border-theme">
            <div className="text-[13px] text-text-base">{t("settings.appearance.translucentSidebar")}</div>
            <ToggleSwitch checked={activeDetails.translucentSidebar} onChange={() => updateThemeDetails(isEditingDark, { translucentSidebar: !activeDetails.translucentSidebar })} />
          </div>
          <div className="flex items-center justify-between px-4 py-3">
            <div className="text-[13px] text-text-base">{t("settings.appearance.contrast")}</div>
            <div className="flex items-center space-x-3">
              <input type="range" min="0" max="100" value={activeDetails.contrast} onChange={(e) => updateThemeDetails(isEditingDark, { contrast: parseInt(e.target.value) })} className="w-32 accent-text-base" />
              <div className="text-[12px] text-text-secondary w-6 text-right">{activeDetails.contrast}</div>
            </div>
          </div>
        </div>
      </div>

      {/* 其他 UI 选项 */}
      <div className="border-t border-border-theme bg-white">
        <div className="flex items-center justify-between p-4 border-b border-border-theme">
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.appearance.usePointerCursor")}</div>
            <div className="text-[12px] text-text-secondary">{t("settings.appearance.usePointerCursorDesc")}</div>
          </div>
          <ToggleSwitch checked={pointerCursor} onChange={() => setPointerCursor(!pointerCursor)} />
        </div>

        <div className="flex items-center justify-between p-4 border-b border-border-theme">
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.appearance.reduceMotion")}</div>
            <div className="text-[12px] text-text-secondary">{t("settings.appearance.reduceMotionDesc")}</div>
          </div>
          <SegmentedControl 
            options={[
              {label: t("settings.appearance.system"), value: 'system'}, 
              {label: t("settings.appearance.on"), value: 'on'}, 
              {label: t("settings.appearance.off"), value: 'off'}
            ]} 
            value={reduceMotion} 
            onChange={(val) => setReduceMotion(val)} 
          />
        </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.appearance.uiFontSize")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.appearance.uiFontSizeDesc")}</div>
            </div>
            <div className="flex items-center">
              <div className="w-12 py-1 bg-white border border-border-theme rounded-md text-center text-[13px] text-text-base shadow-sm">14</div>
              <span className="text-[12px] text-text-secondary ml-2">px</span>
            </div>
          </div>

          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.appearance.codeFontSize")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.appearance.codeFontSizeDesc")}</div>
            </div>
            <div className="flex items-center">
              <div className="w-12 py-1 bg-white border border-border-theme rounded-md text-center text-[13px] text-text-base shadow-sm">12</div>
              <span className="text-[12px] text-text-secondary ml-2">px</span>
            </div>
          </div>

          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.appearance.diffMarkers")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.appearance.diffMarkersDesc")}</div>
            </div>
            <SegmentedControl 
              options={[
                {label: t("settings.appearance.color"), value: 'color'}, 
                {label: '+/-', value: 'symbols'}
              ]} 
              value={diffMarker} 
              onChange={(val) => setDiffMarker(val)} 
            />
          </div>
        </div>
      </div>
    </div>

    {/* Section: 宠物 */}
      <div className="mb-6 max-w-[700px]">
        <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white overflow-hidden">
          <div 
            className="flex items-center justify-between p-4 cursor-pointer hover:bg-gray-50 transition-colors border-b border-border-theme"
            onClick={() => setIsPetExpanded(!isPetExpanded)}
          >
            <div>
              <div className="text-[15px] font-medium text-text-base mb-1">{t("settings.appearance.pet")}</div>
              <div className="text-[13px] text-text-secondary">{t("settings.appearance.selectedPet")}</div>
            </div>
            <FontAwesomeIcon icon={["fas", isPetExpanded ? "chevron-up" : "chevron-down"]} className="text-[12px] text-text-secondary" />
          </div>
          
          {isPetExpanded && (
            <>
              <div className="p-4 bg-gray-50/50 border-b border-border-theme flex justify-end space-x-2">
                <button className="px-3 py-1.5 bg-white border border-border-theme rounded-md text-[12px] font-medium text-text-base hover:bg-gray-50 transition-colors shadow-sm">
                  {t("settings.appearance.createPet")}
                </button>
                <button className="px-3 py-1.5 bg-white border border-border-theme rounded-md text-[12px] font-medium text-text-base hover:bg-gray-50 transition-colors shadow-sm">
                  {t("settings.appearance.refresh")}
                </button>
                <button className="px-3 py-1.5 bg-white border border-border-theme rounded-md text-[12px] font-medium text-text-base hover:bg-gray-50 transition-colors shadow-sm">
                  {t("settings.appearance.wakePet")}
                </button>
              </div>

              <div className="p-4 bg-white space-y-3">
                <PetItem 
                  name="Codex" 
                  desc="The original Codex companion." 
                  icon={["fas", "robot"]} 
                  iconColor="text-blue-500" 
                  selected={true} 
                />
                <PetItem 
                  name="Dewey" 
                  desc="A tidy duck for calm workspace days." 
                  icon={["fas", "cloud"]} 
                  iconColor="text-cyan-500" 
                  selected={false} 
                />
                <PetItem 
                  name="Fireball" 
                  desc="Hot path energy for fast iteration." 
                  icon={["fas", "bullseye"]} 
                  iconColor="text-orange-500" 
                  selected={false} 
                />
                <PetItem 
                  name="Rocky" 
                  desc="A steady rock when the diff gets large." 
                  icon={["fas", "cube"]} 
                  iconColor="text-stone-500" 
                  selected={false} 
                />
                <PetItem 
                  name="Seedy" 
                  desc="Small green shoots for new ideas." 
                  icon={["fas", "leaf"]} 
                  iconColor="text-green-500" 
                  selected={false} 
                />
                <PetItem 
                  name="Stacky" 
                  desc="A balanced stack for deep work." 
                  icon={["fas", "layer-group"]} 
                  iconColor="text-purple-500" 
                  selected={false} 
                />
              </div>
            </>
          )}
        </div>
      </div>
    </>
  );
}
