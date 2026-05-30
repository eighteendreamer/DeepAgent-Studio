import { useState, useRef, useEffect } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";

const MODELS = [
  { name: "灵思API", version: "5.5 中" },
  { name: "灵思API", version: "5.5 Pro" },
  { name: "DeepSeek", version: "V3" },
  { name: "Claude 3.5", version: "Sonnet" },
  { name: "GPT-4o", version: "" },
];

interface Props {
  value: string;
  onChange: (v: string) => void;
  onSubmit: () => void;
  placeholder?: string;
  reviewIcon?: "check" | "history";
}

export function Composer({ value, onChange, onSubmit, placeholder, reviewIcon = "check" }: Props) {
  const { t } = useTranslation();
  const [isModelDropdownOpen, setIsModelDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const [selectedModel, setSelectedModel] = useState(MODELS[0]);

  // Function to translate model name and version
  const getTranslatedModel = (m: typeof MODELS[0]) => {
    let name = m.name;
    let version = m.version;
    if (m.name === "灵思API") name = t("composer.models.lingsiAPI");
    if (m.version === "5.5 中") version = t("composer.models.lingsiAPI_5_5_zh");
    return { name, version };
  };

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsModelDropdownOpen(false);
      }
    };
    if (isModelDropdownOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isModelDropdownOpen]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      onSubmit();
    }
  };

  return (
    <div className="w-full border border-border-theme rounded-xl shadow-[0_2px_10px_rgba(0,0,0,0.02)] bg-white p-3 flex flex-col transition-all focus-within:border-gray-300 focus-within:shadow-md">
      <textarea
        className="w-full min-h-[60px] max-h-[200px] text-text-base placeholder-gray-400 text-sm bg-transparent"
        placeholder={placeholder ?? t("composer.placeholder")}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
      />

      <div className="flex items-center justify-between mt-2 pt-1">
        <div className="flex items-center space-x-2">
          <button className="w-7 h-7 rounded flex items-center justify-center text-text-secondary hover:bg-gray-100 transition-colors">
            <FontAwesomeIcon icon={["fas", "plus"]} />
          </button>
          <div className="flex items-center text-blue-500 text-xs font-medium cursor-pointer hover:bg-blue-50 px-2 py-1.5 rounded transition-colors">
            <FontAwesomeIcon
              icon={reviewIcon === "check" ? ["far", "circle-check"] : ["fas", "clock-rotate-left"]}
              className="mr-1.5"
            />
            {t("composer.autoReview")}
            <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1 text-[10px]" />
          </div>
        </div>

        <div className="flex items-center space-x-2">
          <div className="relative" ref={dropdownRef}>
            <div 
              className="flex items-center bg-gray-50 border border-border-theme rounded-full px-3 py-1 cursor-pointer hover:bg-gray-100 transition-colors text-xs text-text-base"
              onClick={() => setIsModelDropdownOpen(!isModelDropdownOpen)}
            >
              {getTranslatedModel(selectedModel).name} {getTranslatedModel(selectedModel).version && <span className="text-text-secondary ml-1.5">{getTranslatedModel(selectedModel).version}</span>}
              <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-2 text-[10px] text-text-secondary" />
            </div>

            {/* Model Dropdown */}
            {isModelDropdownOpen && (
              <div className="absolute bottom-full right-0 mb-2 w-[220px] bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-50 overflow-hidden py-1">
                <div className="px-3 py-2 text-[11px] text-text-secondary font-medium">{t("composer.selectModel")}</div>
                <div className="flex-1 max-h-[240px] overflow-y-auto py-1">
                  {MODELS.map((m, i) => (
                    <div
                      key={i}
                      className="flex items-center justify-between px-4 py-2 hover:bg-gray-100 cursor-pointer text-[13px] text-text-base group transition-colors"
                      onClick={() => {
                        setSelectedModel(m);
                        setIsModelDropdownOpen(false);
                      }}
                    >
                      <div className="flex items-center">
                        <span className="font-medium">{getTranslatedModel(m).name}</span>
                        {m.version && <span className="text-text-secondary ml-1.5 text-[12px]">{getTranslatedModel(m).version}</span>}
                      </div>
                      {selectedModel.name === m.name && selectedModel.version === m.version && (
                        <FontAwesomeIcon icon={["fas", "check"]} className="text-text-secondary text-[11px]" />
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
          <button
            onClick={onSubmit}
            className="w-8 h-8 rounded-full bg-gray-400 text-white flex items-center justify-center hover:bg-primary transition-colors cursor-pointer"
          >
            <FontAwesomeIcon icon={["fas", "arrow-up"]} />
          </button>
        </div>
      </div>
    </div>
  );
}
