import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useEffect, useState } from "react";
import { getSettings, setSandboxMode, type SandboxMode } from "../../api";
import { message } from "../message";

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

interface DropdownOption {
  title: string;
  description: string;
}

function ComplexDropdown({ 
  options, 
  selectedTitle, 
  onChange 
}: { 
  options: DropdownOption[], 
  selectedTitle: string, 
  onChange: (title: string) => void 
}) {
  const [isOpen, setIsOpen] = useState(false);

  return (
    <div className="relative">
      <div 
        className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors w-[220px] justify-between"
        onClick={() => setIsOpen(!isOpen)}
        onBlur={() => setTimeout(() => setIsOpen(false), 200)}
        tabIndex={0}
      >
        <span className="text-[12px] font-medium text-text-base">{selectedTitle}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
      </div>

      {isOpen && (
        <div className="absolute top-full right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-lg z-20 py-2 w-[300px]">
          {options.map((opt) => (
            <div 
              key={opt.title}
              className="px-4 py-2 hover:bg-gray-50 cursor-pointer flex items-center justify-between"
              onMouseDown={(e) => {
                e.preventDefault(); // Prevent blur
                onChange((opt as any).displayTitle || opt.title);
                setIsOpen(false);
              }}
            >
              <div className="flex-1 pr-4">
                <div className="text-[13px] font-medium text-text-base mb-0.5">{(opt as any).displayTitle || opt.title}</div>
                <div className="text-[12px] text-text-secondary leading-snug">{opt.description}</div>
              </div>
              <div className="w-4 flex justify-end">
                {selectedTitle === ((opt as any).displayTitle || opt.title) && (
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

import { useTranslation } from "react-i18next";

export function ConfigSettings() {
  const { t } = useTranslation();
  const [dependencies, setDependencies] = useState(true);
  const [approvalStrategy, setApprovalStrategy] = useState("onDemand");
  const [sandboxSetting, setSandboxSetting] = useState<SandboxMode>("workspace_write");

  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((settings) => {
        if (!cancelled && settings?.sandbox_mode) {
          setSandboxSetting(settings.sandbox_mode);
        }
      })
      .catch((e) => console.error("get_settings failed:", e));
    return () => {
      cancelled = true;
    };
  }, []);

  const approvalOptions = [
    { title: "untrusted", description: t("settings.config.untrustedDesc"), displayTitle: t("settings.config.untrusted") },
    { title: "onFailure", description: t("settings.config.onFailureDesc"), displayTitle: t("settings.config.onFailure") },
    { title: "onDemand", description: t("settings.config.onDemandDesc"), displayTitle: t("settings.config.onDemand") },
    { title: "never", description: t("settings.config.neverDesc"), displayTitle: t("settings.config.never") }
  ];

  const sandboxOptions = [
    { title: "read_only", description: t("settings.config.readOnlyDesc"), displayTitle: t("settings.config.readOnly") },
    { title: "workspace_write", description: t("settings.config.workspaceWriteDesc"), displayTitle: t("settings.config.workspaceWrite") },
    { title: "full_access", description: t("settings.config.fullAccessDesc"), displayTitle: t("settings.config.fullAccess") }
  ];

  return (
    <>
      <div className="mb-10">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.config.title")}</h1>
        <div className="text-[13px] text-text-secondary flex items-center">
          {t("settings.config.desc")} <a href="#" className="text-blue-500 hover:underline ml-1">{t("settings.config.learnMore")}</a>
        </div>
      </div>

      {/* Section: 自定义 config.toml 设置 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-6">{t("settings.config.customSettings")}</h2>
        
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center bg-gray-100 hover:bg-gray-200 border border-border-theme rounded-lg px-3 py-1.5 cursor-pointer transition-colors">
            <span className="text-[12px] font-medium text-text-base mr-2">{t("settings.config.userConfig")}</span>
            <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-text-secondary" />
          </div>
          <div className="flex items-center text-[12px] text-text-secondary hover:text-text-base cursor-pointer transition-colors">
            {t("settings.config.openConfig")} <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="ml-1.5 text-[10px]" />
          </div>
        </div>

        <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.approvalStrategy")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.approvalDesc")}</div>
            </div>
            <ComplexDropdown options={approvalOptions} selectedTitle={approvalOptions.find(o => o.title === approvalStrategy)?.displayTitle || ""} onChange={(t) => {
              const opt = approvalOptions.find(o => o.displayTitle === t);
              if (opt) setApprovalStrategy(opt.title);
            }} />
          </div>
          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.sandboxSettings")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.sandboxDesc")}</div>
            </div>
            <ComplexDropdown options={sandboxOptions} selectedTitle={sandboxOptions.find(o => o.title === sandboxSetting)?.displayTitle || ""} onChange={async (t) => {
              const opt = sandboxOptions.find(o => o.displayTitle === t);
              if (!opt) return;
              const next = opt.title as SandboxMode;
              const previous = sandboxSetting;
              setSandboxSetting(next);
              try {
                const view = await setSandboxMode(next);
                setSandboxSetting(view.sandbox_mode);
              } catch (e) {
                setSandboxSetting(previous);
                message.error("沙箱设置保存失败");
                console.error("set_sandbox_mode failed:", e);
              }
            }} />
          </div>
        </div>
      </div>

      {/* Section: 工作空间依赖项 */}
      <div className="mb-12 max-w-[700px]">
        <h2 className="text-[15px] font-medium text-text-base mb-4">{t("settings.config.dependencies")}</h2>
        <div className="border border-border-theme rounded-xl overflow-hidden shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white">
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="text-[14px] font-medium text-text-base">{t("settings.config.currentVersion")}</div>
            <div className="text-[13px] text-text-secondary font-mono">26.521.10419</div>
          </div>
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.codexDependencies")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.codexDependenciesDesc")}</div>
            </div>
            <ToggleSwitch checked={dependencies} onChange={() => setDependencies(!dependencies)} />
          </div>
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.diagnose")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.diagnoseDesc")}</div>
            </div>
            <button className="flex items-center px-4 py-1.5 bg-gray-50 hover:bg-gray-100 border border-border-theme rounded-md text-[12px] font-medium text-text-base transition-colors shadow-sm">
              <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="mr-2 text-[11px] text-text-secondary" /> {t("settings.config.diagnoseBtn")}
            </button>
          </div>
          <div className="flex items-center justify-between p-4">
            <div>
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.config.resetWorkspace")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.config.resetDesc")}</div>
            </div>
            <button className="flex items-center px-4 py-1.5 bg-red-50/50 hover:bg-red-50 border border-red-200 rounded-md text-[12px] font-medium text-red-500 transition-colors shadow-sm">
              <FontAwesomeIcon icon={["fas", "download"]} className="mr-2 text-[11px]" /> {t("settings.config.reinstall")}
            </button>
          </div>
        </div>
      </div>
    </>
  );
}
