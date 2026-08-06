import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState } from "react";

import { useTranslation } from "react-i18next";

export function ComputerSettings() {
  const { t } = useTranslation();
  const [isInstallModalOpen, setIsInstallModalOpen] = useState(false);

  return (
    <>
      <div className="pb-20">
        <div className="mb-10 max-w-[700px]">
          <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.computer.title")}</h1>
          <div className="text-[13px] text-text-secondary">
            {t("settings.computer.desc")}
          </div>
        </div>

        <div className="max-w-[700px]">
          <h2 className="text-[14px] font-medium text-text-base mb-3">{t("settings.computer.control")}</h2>
          
          <div className="border border-border-theme rounded-xl p-4 flex items-center justify-between bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] mb-8">
            <div className="flex items-center">
              <div className="relative mr-4">
                <div className="w-10 h-10 flex items-center justify-center">
                  <FontAwesomeIcon icon={["fab", "chrome"]} className="text-[32px] text-blue-500" style={{
                    // Approximate Chrome colors with a gradient or just blue for simplicity
                    background: "-webkit-linear-gradient(45deg, #4285F4, #34A853, #FBBC05, #EA4335)",
                    WebkitBackgroundClip: "text",
                    WebkitTextFillColor: "transparent"
                  }} />
                </div>
                {/* Small overlay icon */}
                <div className="absolute bottom-0 right-0 translate-x-1 translate-y-1">
                  <FontAwesomeIcon icon={["fas", "puzzle-piece"]} className="text-[16px] text-gray-500 drop-shadow-sm" />
                </div>
              </div>
              <div>
                <div className="text-[14px] font-medium text-text-base mb-0.5">Google Chrome</div>
                <div className="text-[12px] text-text-secondary flex items-center">
                  <span className="w-1.5 h-1.5 rounded-full bg-red-500 mr-1.5"></span>
                  {t("settings.computer.extensionNotConnected")}
                </div>
              </div>
            </div>
            <button 
              className="px-4 py-1.5 bg-black/5 hover:bg-black/5 rounded-full text-[12px] font-medium text-text-base transition-colors"
              onClick={() => setIsInstallModalOpen(true)}
            >
              {t("settings.computer.install")}
            </button>
          </div>

          <h2 className="text-[14px] font-medium text-text-base mb-3">{t("settings.computer.alwaysAllowed")}</h2>
          
          <div className="border border-border-theme rounded-xl p-4 bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] flex justify-center items-center h-[60px]">
            <span className="text-[13px] text-text-secondary">{t("settings.computer.none")}</span>
          </div>
        </div>
      </div>

      {/* Installation Modal */}
      {isInstallModalOpen && (
        <div className="fixed inset-0 bg-white/60 backdrop-blur-sm z-50 flex justify-center items-center">
          <div className="bg-elevated-bg rounded-2xl shadow-[0_6px_24px_rgba(0,0,0,0.10)] w-[500px] flex flex-col max-h-[90vh] overflow-hidden relative">
            
            {/* Close Button */}
            <button 
              className="absolute top-4 right-4 w-6 h-6 flex items-center justify-center text-blue-500 hover:bg-blue-50 rounded transition-colors"
              onClick={() => setIsInstallModalOpen(false)}
            >
              <FontAwesomeIcon icon={["fas", "xmark"]} className="text-[14px]" />
            </button>

            {/* Header */}
            <div className="pt-8 pb-6 flex flex-col items-center">
              <div className="flex items-center space-x-4 mb-4">
                <div className="w-12 h-12 flex items-center justify-center overflow-hidden">
                  <img src="/logo.png" alt="Logo" className="w-full h-full object-contain" />
                </div>
                <div className="flex space-x-1">
                  <span className="w-1 h-1 rounded-full bg-gray-300"></span>
                  <span className="w-1 h-1 rounded-full bg-gray-300"></span>
                  <span className="w-1 h-1 rounded-full bg-gray-300"></span>
                </div>
                <div className="relative">
                  <div className="w-12 h-12 flex items-center justify-center">
                    <FontAwesomeIcon icon={["fab", "chrome"]} className="text-[40px] text-blue-500" style={{
                      background: "-webkit-linear-gradient(45deg, #4285F4, #34A853, #FBBC05, #EA4335)",
                      WebkitBackgroundClip: "text",
                      WebkitTextFillColor: "transparent"
                    }} />
                  </div>
                  <div className="absolute bottom-0 right-0 translate-x-1 translate-y-1">
                    <FontAwesomeIcon icon={["fas", "puzzle-piece"]} className="text-[20px] text-gray-500 drop-shadow-sm" />
                  </div>
                </div>
              </div>
              <h2 className="text-[20px] font-semibold text-text-base mb-1">{t("settings.computer.installChrome")}</h2>
              <div className="text-[13px] text-text-secondary">{t("settings.computer.developedBy")}</div>
            </div>

            {/* Scrollable Content Area */}
            <div className="flex-1 overflow-y-auto px-6 pb-6">
              <div className="border border-border-theme rounded-xl p-5 bg-white">
                <div className="mb-4">
                  <div className="flex items-center mb-1">
                    <span className="text-[14px] font-medium text-text-base mr-2">Chrome</span>
                    <span className="px-2 py-0.5 border border-border-theme rounded text-[11px] text-text-secondary">openai-bundled</span>
                  </div>
                  <div className="text-[12px] text-text-secondary mb-0.5">{t("settings.computer.providedBy")}</div>
                  <div className="text-[12px] text-text-secondary">{t("settings.computer.category")}</div>
                </div>

                <div className="mb-4">
                  <h3 className="text-[13px] font-medium text-text-base mb-2">{t("settings.computer.about")}</h3>
                  <div className="text-[12px] text-text-secondary leading-relaxed">
                    Chrome lets Codex use your Chrome browser for tasks that need your existing browser state, including open tabs, page content, and websites you're already signed into. It can navigate, view, click, type, and take screenshots while working. You stay in control: Codex asks before interacting with new sites, you can stop actions at any time, and you can manage or remove Chrome access in settings. Browser content may include sensitive information from logged-in sites. Browser data from using this plugin may be used for training, subject to your OpenAI account data controls.
                  </div>
                </div>

                <div className="mb-4">
                  <h3 className="text-[13px] font-medium text-text-base mb-2">{t("settings.computer.includes")}</h3>
                  <div className="mb-2">
                    <div className="text-[12px] text-text-secondary mb-1">{t("settings.computer.browserExtension")}</div>
                    <span className="px-2 py-1 bg-gray-50 border border-gray-200 rounded-md text-[12px] text-text-secondary inline-block">Codex Chrome Extension</span>
                  </div>
                  <div>
                    <div className="text-[12px] text-text-secondary mb-1">{t("settings.computer.skills")}</div>
                    <span className="px-2 py-1 bg-gray-50 border border-gray-200 rounded-md text-[12px] text-text-secondary inline-block">Chrome</span>
                  </div>
                </div>

                <div>
                  <h3 className="text-[13px] font-medium text-text-base mb-2">{t("settings.computer.features")}</h3>
                  <div className="flex space-x-2">
                    <span className="px-2 py-1 bg-gray-50 border border-gray-200 rounded-md text-[12px] text-text-secondary">Interactive</span>
                    <span className="px-2 py-1 bg-gray-50 border border-gray-200 rounded-md text-[12px] text-text-secondary">Read</span>
                  </div>
                </div>
              </div>
            </div>

            {/* Footer */}
            <div className="p-4 border-t border-border-theme bg-white">
              <button 
                className="w-full py-2.5 bg-black hover:bg-gray-800 text-white rounded-xl text-[14px] font-medium transition-colors"
                onClick={() => setIsInstallModalOpen(false)}
              >
                {t("settings.computer.installChrome")}
              </button>
            </div>

          </div>
        </div>
      )}
    </>
  );
}
