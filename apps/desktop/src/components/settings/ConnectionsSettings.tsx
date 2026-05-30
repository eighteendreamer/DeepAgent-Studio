import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState } from "react";

import { useTranslation } from "react-i18next";

export function ConnectionsSettings() {
  const { t } = useTranslation();
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [modalStep, setModalStep] = useState<"template" | "manual">("template");
  const [authType, setAuthType] = useState<"none" | "file">("none");

  const openModal = () => {
    setIsModalOpen(true);
    setModalStep("template");
  };

  const closeModal = () => {
    setIsModalOpen(false);
  };

  return (
    <>
      <div className="mb-8">
        <h1 className="text-2xl font-semibold text-text-base">{t("settings.connections.title")}</h1>
      </div>

      <div className="mb-6 max-w-[800px]">
        <h2 className="text-[13px] font-medium text-text-base mb-4">{t("settings.connections.sshConnections")}</h2>
        
        {/* Empty State Card */}
        <div className="border border-border-theme rounded-xl bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] p-10 flex flex-col items-center justify-center">
          <div className="flex items-center text-gray-700 text-2xl mb-3 space-x-2">
            <FontAwesomeIcon icon={["fas", "laptop"]} />
            <span className="text-sm font-bold tracking-widest relative -top-1">...</span>
            <FontAwesomeIcon icon={["fas", "server"]} />
          </div>
          <div className="text-[13px] text-text-secondary mb-4">{t("settings.connections.sshDesc")}</div>
          <button 
            className="px-4 py-1.5 bg-gray-100 hover:bg-gray-200 border border-transparent rounded-full text-[13px] text-text-base font-medium transition-colors"
            onClick={openModal}
          >
            {t("settings.connections.add")}
          </button>
        </div>
      </div>

      {/* Modal Backdrop & Container */}
      {isModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/20">
          <div className="bg-white rounded-2xl shadow-xl w-full max-w-[500px] overflow-hidden">
            
            {/* Modal Header */}
            <div className="flex items-center justify-between px-6 py-4 border-b border-transparent">
              <h3 className="text-lg font-semibold text-text-base">{t("settings.connections.addSsh")}</h3>
              <button 
                className="text-gray-400 hover:text-text-base transition-colors"
                onClick={closeModal}
              >
                <FontAwesomeIcon icon={["fas", "times"]} className="text-[14px]" />
              </button>
            </div>

            {/* Modal Content - Template Step */}
            {modalStep === "template" && (
              <>
                <div className="px-6 py-2 pb-6">
                  <div className="border border-blue-500 bg-blue-50/10 rounded-xl p-3 flex items-center justify-between cursor-pointer">
                    <div className="flex items-center">
                      <FontAwesomeIcon icon={["fas", "laptop"]} className="text-gray-500 mr-4 text-[14px]" />
                      <div>
                        <div className="text-[13px] font-medium text-text-base">github.com</div>
                        <div className="text-[12px] text-text-secondary">ssh.github.com</div>
                      </div>
                    </div>
                    <FontAwesomeIcon icon={["fas", "check"]} className="text-blue-500 text-[12px]" />
                  </div>
                </div>
                <div className="px-6 py-4 border-t border-border-theme flex justify-between items-center bg-gray-50/50">
                  <button 
                    className="text-[13px] text-text-secondary hover:text-text-base transition-colors flex items-center"
                    onClick={() => setModalStep("manual")}
                  >
                    <FontAwesomeIcon icon={["fas", "pen"]} className="mr-2 text-[12px]" /> {t("settings.connections.addManually")}
                  </button>
                  <button className="px-6 py-1.5 bg-black hover:bg-gray-800 text-white rounded-full text-[13px] font-medium transition-colors shadow-sm">
                    {t("settings.connections.add")}
                  </button>
                </div>
              </>
            )}

            {/* Modal Content - Manual Entry Step */}
            {modalStep === "manual" && (
              <>
                <div className="px-6 py-2 pb-6 space-y-4">
                  <div>
                    <label className="block text-[12px] font-medium text-text-base mb-1.5">{t("settings.connections.displayName")}</label>
                    <input 
                      type="text" 
                      className="w-full border border-blue-400 rounded-lg py-1.5 px-3 text-[13px] focus:outline-none focus:border-blue-500 shadow-[0_0_0_1px_rgba(59,130,246,0.2)] bg-white"
                    />
                  </div>
                  
                  <div>
                    <label className="block text-[12px] font-medium text-text-base mb-1.5">{t("settings.connections.hostname")}</label>
                    <input 
                      type="text" 
                      placeholder={t("settings.connections.hostPlaceholder")}
                      className="w-full border border-border-theme rounded-lg py-1.5 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
                    />
                  </div>

                  <div>
                    <label className="block text-[12px] font-medium text-text-base mb-1.5">{t("settings.connections.sshPort")} <span className="text-gray-400 font-normal">{t("settings.connections.optional")}</span></label>
                    <input 
                      type="text" 
                      className="w-full border border-border-theme rounded-lg py-1.5 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
                    />
                  </div>

                  {/* Auth Type Segmented Control */}
                  <div className="flex border border-border-theme rounded-full overflow-hidden bg-gray-100 p-0.5 mt-2">
                    <button 
                      className={`flex-1 py-1.5 text-[12px] font-medium rounded-full transition-colors ${authType === 'none' ? 'bg-white text-text-base shadow-sm' : 'text-text-secondary hover:text-text-base'}`}
                      onClick={() => setAuthType("none")}
                    >
                      {t("settings.connections.noAuth")}
                    </button>
                    <button 
                      className={`flex-1 py-1.5 text-[12px] font-medium rounded-full transition-colors ${authType === 'file' ? 'bg-white text-text-base shadow-sm' : 'text-text-secondary hover:text-text-base'}`}
                      onClick={() => setAuthType("file")}
                    >
                      {t("settings.connections.identityFile")}
                    </button>
                  </div>

                  {authType === "file" && (
                    <div className="pt-2">
                      <label className="block text-[12px] font-medium text-text-base mb-1.5">{t("settings.connections.identityFilePath")}</label>
                      <input 
                        type="text" 
                        className="w-full border border-border-theme rounded-lg py-1.5 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
                      />
                    </div>
                  )}
                </div>

                <div className="px-6 py-4 flex justify-end items-center space-x-4 bg-gray-50/50">
                  <button 
                    className="text-[13px] text-text-secondary hover:text-text-base transition-colors"
                    onClick={() => setModalStep("template")}
                  >
                    {t("settings.connections.cancel")}
                  </button>
                  <button className="px-6 py-1.5 bg-black hover:bg-gray-800 text-white rounded-full text-[13px] font-medium transition-colors shadow-sm">
                    {t("settings.connections.save")}
                  </button>
                </div>
              </>
            )}

          </div>
        </div>
      )}
    </>
  );
}
