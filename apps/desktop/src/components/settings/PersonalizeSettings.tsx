import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";

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

function PersonalityDropdown() {
  const { t } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);
  const [selected, setSelected] = useState<"affable" | "pragmatic">("affable");
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  return (
    <div className="relative" ref={dropdownRef}>
      <div 
        className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors w-48 justify-between"
        onClick={() => setIsOpen(!isOpen)}
      >
        <span className="text-[12px] font-medium text-text-base mr-3">{selected === "affable" ? t("settings.personalize.affable") : t("settings.personalize.pragmatic")}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
      </div>

      {isOpen && (
        <div className="absolute top-full mt-1 right-0 w-64 bg-white border border-border-theme rounded-xl shadow-lg z-10 overflow-hidden">
          <div 
            className="p-3 hover:bg-gray-50 cursor-pointer border-b border-border-theme flex items-center justify-between"
            onClick={() => { setSelected("affable"); setIsOpen(false); }}
          >
            <div>
              <div className="text-[13px] font-medium text-text-base mb-0.5">{t("settings.personalize.affable")}</div>
              <div className="text-[11px] text-text-secondary">{t("settings.personalize.affableDesc")}</div>
            </div>
            {selected === "affable" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />}
          </div>
          <div 
            className="p-3 hover:bg-gray-50 cursor-pointer flex items-center justify-between"
            onClick={() => { setSelected("pragmatic"); setIsOpen(false); }}
          >
            <div>
              <div className="text-[13px] font-medium text-text-base mb-0.5">{t("settings.personalize.pragmatic")}</div>
              <div className="text-[11px] text-text-secondary">{t("settings.personalize.pragmaticDesc")}</div>
            </div>
            {selected === "pragmatic" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />}
          </div>
        </div>
      )}
    </div>
  );
}

export function PersonalizeSettings() {
  const { t } = useTranslation();
  const [customInstruction, setCustomInstruction] = useState("");
  const [enableMemory, setEnableMemory] = useState(false);
  const [skipToolMemory, setSkipToolMemory] = useState(false);

  return (
    <>
      <div className="mb-10">
        <h1 className="text-2xl font-semibold text-text-base">{t("settings.personalize.title")}</h1>
      </div>

      {/* Section: 个性 */}
      <div className="mb-8 max-w-[700px]">
        <div className="border border-border-theme rounded-xl overflow-visible shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.personalize.personality")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.personalize.personalityDesc")}</div>
            </div>
            <PersonalityDropdown />
          </div>
        </div>
      </div>

      {/* Section: 自定义指令 */}
      <div className="mb-12 max-w-[700px]">
        <div className="mb-2">
          <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.personalize.customInstructions")}</div>
          <div className="text-[12px] text-text-secondary">
            {t("settings.personalize.customInstructionsDesc")} <a href="#" className="text-blue-500 hover:underline">{t("settings.personalize.learnMore")}</a>
          </div>
        </div>
        
        <div className="border border-border-theme rounded-xl overflow-hidden bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] flex flex-col">
          <textarea 
            className="w-full h-[300px] p-4 text-[13px] text-text-base resize-none focus:outline-none placeholder-gray-400 bg-transparent"
            placeholder={t("settings.personalize.customInstructionsPlaceholder")}
            value={customInstruction}
            onChange={(e) => setCustomInstruction(e.target.value)}
          ></textarea>
        </div>
        <div className="mt-3 flex justify-end">
          <button 
            className={`px-4 py-1.5 rounded-full text-[13px] font-medium transition-colors ${customInstruction.trim() !== "" ? 'bg-blue-500 text-white hover:bg-blue-600 shadow-sm' : 'bg-gray-300 text-white cursor-not-allowed'}`}
            disabled={customInstruction.trim() === ""}
          >
            {t("settings.personalize.save")}
          </button>
        </div>
      </div>

      {/* Section: 记忆（实验性） */}
      <div className="mb-12 max-w-[700px]">
        <div className="mb-4">
          <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.personalize.memory")}</div>
          <div className="text-[12px] text-text-secondary">
            {t("settings.personalize.memoryDesc")} <a href="#" className="text-blue-500 hover:underline">{t("settings.personalize.learnMore")}</a>
          </div>
        </div>
        
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.personalize.enableMemory")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.personalize.enableMemoryDesc")}</div>
            </div>
            <ToggleSwitch checked={enableMemory} onChange={() => setEnableMemory(!enableMemory)} />
          </div>
          
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.personalize.skipToolMemory")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.personalize.skipToolMemoryDesc")}</div>
            </div>
            <ToggleSwitch checked={skipToolMemory} onChange={() => setSkipToolMemory(!skipToolMemory)} />
          </div>
          
          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.personalize.resetMemory")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.personalize.resetMemoryDesc")}</div>
            </div>
            <button className="flex items-center px-4 py-1.5 bg-red-50/50 hover:bg-red-50 border border-red-200 rounded-md text-[12px] font-medium text-red-500 transition-colors shadow-sm">
              {t("settings.personalize.reset")}
            </button>
          </div>
        </div>
      </div>
    </>
  );
}
