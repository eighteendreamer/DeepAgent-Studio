import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState } from "react";

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

import { useTranslation } from "react-i18next";

export function BrowserSettings() {
  const { t } = useTranslation();
  const [browserEnabled, setBrowserEnabled] = useState(true);
  
  // States for dropdowns and expandables
  const [isDataExpanded, setIsDataExpanded] = useState(true);
  const [screenshotOption, setScreenshotOption] = useState("alwaysInclude");
  const [isScreenshotDropdownOpen, setIsScreenshotDropdownOpen] = useState(false);
  
  const [approvalOption, setApprovalOption] = useState("alwaysAsk");
  const [isApprovalDropdownOpen, setIsApprovalDropdownOpen] = useState(false);

  return (
    <div className="pb-20">
      <div className="mb-10 max-w-[700px]">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.browser.title")}</h1>
        <div className="text-[13px] text-text-secondary">
          {t("settings.browser.desc1")} <a href="#" className="text-blue-500 hover:underline">{t("settings.browser.desc2")}</a> {t("settings.browser.desc3")}
        </div>
      </div>

      <div className="mb-10 max-w-[700px]">
        <div className="border border-border-theme rounded-xl p-4 flex items-center justify-between bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <div className="flex items-center">
            <div className="w-10 h-10 bg-white border border-border-theme rounded-lg flex items-center justify-center mr-4 shadow-sm">
              <div className="relative flex items-center justify-center mt-1 mr-1">
                <FontAwesomeIcon icon={["far", "window-maximize"]} className="text-[22px] text-gray-700" />
                <div className="absolute -bottom-1 -right-1.5 bg-white rounded-full pl-0.5 pt-0.5">
                  <FontAwesomeIcon icon={["fas", "arrow-pointer"]} className="text-[12px] text-gray-700 drop-shadow-sm" />
                </div>
              </div>
            </div>
            <div>
              <div className="text-[14px] font-medium text-text-base mb-0.5">{t("settings.browser.title")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.browser.allowControl")}</div>
            </div>
          </div>
          <ToggleSwitch checked={browserEnabled} onChange={() => setBrowserEnabled(!browserEnabled)} />
        </div>
      </div>

      <div className="mb-10 max-w-[700px]">
        <h2 className="text-[14px] font-medium text-text-base mb-3">{t("settings.browser.dataTitle")}</h2>
        <div className="border border-border-theme rounded-xl bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          
          {/* 浏览数据 Header */}
          <div className="p-4 border-b border-border-theme flex items-center justify-between">
            <div>
              <div className="text-[13px] font-medium text-text-base mb-0.5">{t("settings.browser.browsingData")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.browser.clearDataDesc")}</div>
            </div>
            <div className="flex items-center space-x-3">
              <button className="px-4 py-1.5 bg-gray-100 hover:bg-gray-200 rounded-full text-[12px] font-medium text-text-base transition-colors">
                {t("settings.browser.clearAllData")}
              </button>
              <button 
                className="w-6 h-6 flex items-center justify-center text-gray-400 hover:text-text-base transition-colors"
                onClick={() => setIsDataExpanded(!isDataExpanded)}
              >
                <FontAwesomeIcon icon={["fas", isDataExpanded ? "chevron-up" : "chevron-down"]} className="text-[12px]" />
              </button>
            </div>
          </div>

          {/* 浏览数据 Expanded Details */}
          {isDataExpanded && (
            <div className="bg-gray-50/30">
              <div className="px-4 py-3 border-b border-border-theme flex items-center justify-between">
                <span className="text-[13px] text-text-secondary">{t("settings.browser.cookie")}</span>
                <button className="text-[12px] text-text-secondary hover:text-text-base transition-colors">{t("settings.browser.deleteCookie")}</button>
              </div>
              <div className="px-4 py-3 border-b border-border-theme flex items-center justify-between">
                <span className="text-[13px] text-text-secondary">{t("settings.browser.websiteData")}</span>
                <button className="text-[12px] text-text-secondary hover:text-text-base transition-colors">{t("settings.browser.deleteWebsiteData")}</button>
              </div>
              <div className="px-4 py-3 border-b border-border-theme flex items-center justify-between">
                <span className="text-[13px] text-text-secondary">{t("settings.browser.cachedImages")}</span>
                <button className="text-[12px] text-text-secondary hover:text-text-base transition-colors">{t("settings.browser.deleteCachedImages")}</button>
              </div>
            </div>
          )}

          {/* 批注截图 */}
          <div className="p-4 flex items-center justify-between relative">
            <div className="pr-4">
              <div className="text-[13px] font-medium text-text-base mb-0.5">{t("settings.browser.screenshots")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.browser.screenshotsDesc")}</div>
            </div>
            
            {/* Custom Dropdown */}
            <div className="relative">
              <button 
                className="w-[200px] flex items-center justify-between px-3 py-1.5 bg-gray-100 hover:bg-gray-200 rounded-lg text-[13px] font-medium text-text-base transition-colors"
                onClick={() => setIsScreenshotDropdownOpen(!isScreenshotDropdownOpen)}
                onBlur={() => setTimeout(() => setIsScreenshotDropdownOpen(false), 200)}
              >
                <span>{screenshotOption === "alwaysInclude" ? t("settings.browser.alwaysInclude") : t("settings.browser.onlySelected")}</span>
                <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-gray-500" />
              </button>

              {isScreenshotDropdownOpen && (
                <div className="absolute top-full left-0 right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-lg z-10 py-1">
                  {[
                    { value: "alwaysInclude", label: t("settings.browser.alwaysInclude") },
                    { value: "onlySelected", label: t("settings.browser.onlySelected") }
                  ].map(opt => (
                    <button 
                      key={opt.value}
                      className="w-full text-left px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 flex items-center justify-between"
                      onClick={() => {
                        setScreenshotOption(opt.value);
                        setIsScreenshotDropdownOpen(false);
                      }}
                    >
                      {opt.label}
                      {screenshotOption === opt.value && <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px]" />}
                    </button>
                  ))}
                </div>
              )}
            </div>
          </div>

        </div>
      </div>

      <div className="mb-10 max-w-[700px]">
        <h2 className="text-[14px] font-medium text-text-base mb-3">{t("settings.browser.permissions")}</h2>
        
        {/* 审批 */}
        <div className="border border-border-theme rounded-xl p-4 flex justify-between bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] mb-6 items-center">
          <div className="pr-4">
            <div className="text-[13px] font-medium text-text-base mb-0.5">{t("settings.browser.approval")}</div>
            <div className="text-[12px] text-text-secondary">{t("settings.browser.approvalDesc")} <a href="#" className="text-blue-500 hover:underline">{t("settings.browser.learnMore")}</a></div>
          </div>

          {/* Complex Dropdown */}
          <div className="relative">
            <button 
              className="w-[240px] flex items-center justify-between px-3 py-1.5 bg-gray-100 hover:bg-gray-200 rounded-lg text-[13px] font-medium text-text-base transition-colors"
              onClick={() => setIsApprovalDropdownOpen(!isApprovalDropdownOpen)}
              onBlur={() => setTimeout(() => setIsApprovalDropdownOpen(false), 200)}
            >
              <span>{approvalOption === "alwaysAsk" ? t("settings.browser.alwaysAsk") : t("settings.browser.alwaysAllow")}</span>
              <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[10px] text-gray-500" />
            </button>

            {isApprovalDropdownOpen && (
              <div className="absolute top-full left-0 right-0 mt-1 bg-white border border-border-theme rounded-xl shadow-xl z-20 py-2 w-[280px] -ml-10">
                <button 
                  className="w-full text-left px-4 py-2 hover:bg-gray-50"
                  onClick={() => {
                    setApprovalOption("alwaysAsk");
                    setIsApprovalDropdownOpen(false);
                  }}
                >
                  <div className="flex items-center justify-between mb-0.5">
                    <span className="text-[13px] font-medium text-text-base">{t("settings.browser.alwaysAsk")}</span>
                    {approvalOption === "alwaysAsk" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px]" />}
                  </div>
                  <div className="text-[12px] text-text-secondary">{t("settings.browser.askBeforeOpen")}</div>
                </button>
                
                <button 
                  className="w-full text-left px-4 py-2 hover:bg-gray-50"
                  onClick={() => {
                    setApprovalOption("alwaysAllow");
                    setIsApprovalDropdownOpen(false);
                  }}
                >
                  <div className="flex items-center justify-between mb-0.5">
                    <span className="text-[13px] font-medium text-text-base">{t("settings.browser.alwaysAllow")}</span>
                    {approvalOption === "alwaysAllow" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px]" />}
                  </div>
                  <div className="text-[12px] text-text-secondary mb-1">{t("settings.browser.allowWithoutAsking")}</div>
                  <div className="flex items-start text-orange-600 bg-orange-50 p-2 rounded border border-orange-100">
                    <FontAwesomeIcon icon={["fas", "circle-info"]} className="mt-0.5 mr-1.5 text-[12px]" />
                    <span className="text-[11px] leading-snug">{t("settings.browser.highRisk")}</span>
                  </div>
                </button>
              </div>
            )}
          </div>
        </div>

        {/* 已屏蔽的域名 */}
        <div className="mb-6">
          <div className="flex items-center justify-between mb-2">
            <div>
              <div className="text-[13px] font-medium text-text-base mb-0.5">{t("settings.browser.blockedDomains")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.browser.blockedDesc")}</div>
            </div>
            <button className="px-3 py-1.5 bg-gray-50 hover:bg-gray-100 border border-border-theme rounded-lg text-[12px] font-medium text-text-base transition-colors flex items-center">
              <FontAwesomeIcon icon={["fas", "plus"]} className="mr-1.5 text-[10px]" /> {t("settings.browser.add")}
            </button>
          </div>
          <div className="border border-border-theme rounded-xl p-4 bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] flex justify-center items-center h-[60px]">
            <span className="text-[13px] text-text-secondary">{t("settings.browser.noBlocked")}</span>
          </div>
        </div>

        {/* 允许的域名 */}
        <div>
          <div className="flex items-center justify-between mb-2">
            <div>
              <div className="text-[13px] font-medium text-text-base mb-0.5">{t("settings.browser.allowedDomains")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.browser.allowedDesc")}</div>
            </div>
            <button className="px-3 py-1.5 bg-gray-50 hover:bg-gray-100 border border-border-theme rounded-lg text-[12px] font-medium text-text-base transition-colors flex items-center">
              <FontAwesomeIcon icon={["fas", "plus"]} className="mr-1.5 text-[10px]" /> {t("settings.browser.add")}
            </button>
          </div>
          <div className="border border-border-theme rounded-xl p-4 bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] flex justify-center items-center h-[60px]">
            <span className="text-[13px] text-text-secondary">{t("settings.browser.noAllowed")}</span>
          </div>
        </div>

      </div>
    </div>
  );
}
