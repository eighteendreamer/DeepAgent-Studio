import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState } from "react";

type ViewState = "list" | "detail" | "edit";

import { useTranslation } from "react-i18next";

export function EnvSettings() {
  const { t } = useTranslation();
  const [view, setView] = useState<ViewState>("list");
  
  // State for the tabs in editors
  const [setupTab, setSetupTab] = useState("default");
  const [cleanupTab, setCleanupTab] = useState("default");

  // Sample data for the list
  const projects = [
    { name: "CodeSprout_Ai", sub: "" },
    { name: "Resume-Assistant", sub: "eighteendreamer" },
    { name: "Python_selenium_test_Agent", sub: "" },
    { name: "Spark_Technology_Big_Data", sub: "" },
    { name: "图项目", sub: "" },
    { name: "Looker-v2", sub: "" },
    { name: "小红草", sub: "" },
    { name: "weike", sub: "weikei" },
  ];

  if (view === "detail") {
    return (
      <div className="max-w-[700px]">
        {/* Breadcrumb */}
        <div className="absolute top-6 left-16 flex items-center text-[13px] text-text-secondary">
          <button 
            className="hover:text-text-base transition-colors flex items-center"
            onClick={() => setView("list")}
          >
            <FontAwesomeIcon icon={["fas", "arrow-left"]} className="mr-2 text-[12px]" /> {t("settings.env.back")}
          </button>
          <span className="mx-2">&gt;</span>
          <span>{t("settings.env.title")}</span>
          <span className="mx-2">&gt;</span>
          <span className="text-text-base">CodeSprout_Ai</span>
        </div>

        <h1 className="text-2xl font-semibold text-text-base mb-10">{t("settings.env.title")}</h1>

        <div className="mb-8">
          <div className="text-[14px] font-medium text-text-base mb-2">{t("settings.env.project")}</div>
          <div className="border border-border-theme rounded-xl p-3 flex items-center bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
            <FontAwesomeIcon icon={["far", "folder"]} className="text-gray-400 mr-3 text-[14px]" />
            <div>
              <div className="text-[13px] font-medium text-text-base">CodeSprout_Ai</div>
              <div className="text-[12px] text-text-secondary">G:\Code_Warehouse\CodeSprout_Ai</div>
            </div>
          </div>
        </div>

        <div>
          <div className="text-[14px] font-medium text-text-base mb-2">{t("settings.env.envDetails")}</div>
          <div className="border border-border-theme rounded-xl p-4 bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] mb-4">
            <div className="text-[13px] text-text-secondary">{t("settings.env.noLocalEnv")}</div>
          </div>
          <div className="flex justify-end">
            <button 
              className="px-4 py-1.5 bg-black hover:bg-gray-800 text-white rounded-full text-[13px] font-medium transition-colors shadow-sm"
              onClick={() => setView("edit")}
            >
              {t("settings.env.createLocalEnv")}
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (view === "edit") {
    return (
      <div className="max-w-[700px]">
        {/* Breadcrumb */}
        <div className="absolute top-6 left-16 flex items-center text-[13px] text-text-secondary">
          <button 
            className="hover:text-text-base transition-colors flex items-center"
            onClick={() => setView("list")}
          >
            <FontAwesomeIcon icon={["fas", "arrow-left"]} className="mr-2 text-[12px]" /> {t("settings.env.back")}
          </button>
          <span className="mx-2">&gt;</span>
          <span>{t("settings.env.title")}</span>
          <span className="mx-2">&gt;</span>
          <span className="cursor-pointer hover:underline" onClick={() => setView("detail")}>CodeSprout_Ai</span>
          <span className="mx-2">&gt;</span>
          <span className="text-text-base">{t("settings.env.edit")}</span>
        </div>

        <h1 className="text-2xl font-semibold text-text-base mb-10">{t("settings.env.title")}</h1>

        <div className="space-y-8 pb-20">
          <div>
            <div className="text-[14px] font-medium text-text-base mb-2">{t("settings.env.localEnv")}</div>
            <div className="border border-border-theme rounded-xl p-3 flex items-center bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
              <FontAwesomeIcon icon={["far", "folder"]} className="text-gray-400 mr-3 text-[14px]" />
              <div>
                <div className="text-[13px] font-medium text-text-base">CodeSprout_Ai</div>
                <div className="text-[12px] text-text-secondary">G:\Code_Warehouse\CodeSprout_Ai</div>
              </div>
            </div>
          </div>

          <div>
            <div className="text-[14px] font-medium text-text-base mb-2">{t("settings.env.name")}</div>
            <input 
              type="text" 
              defaultValue="CodeSprout_Ai"
              className="w-[300px] border border-border-theme rounded-lg py-1.5 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white"
            />
          </div>

          <div>
            <div className="mb-2">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.env.setupScript")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.env.setupScriptDesc")}</div>
            </div>
            
            <div className="border border-border-theme rounded-xl overflow-hidden bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
              <div className="flex items-center justify-between px-4 py-2 border-b border-border-theme bg-gray-50/50">
                <div className="flex space-x-4 text-[12px]">
                  {[
                    { value: "default", label: t("settings.env.default") },
                    { value: "macOS", label: "macOS" },
                    { value: "Linux", label: "Linux" },
                    { value: "Windows", label: "Windows" }
                  ].map(tab => (
                    <button 
                      key={tab.value}
                      className={`transition-colors ${setupTab === tab.value ? 'font-medium text-text-base' : 'text-text-secondary hover:text-text-base'}`}
                      onClick={() => setSetupTab(tab.value)}
                    >
                      {tab.label}
                    </button>
                  ))}
                </div>
                <button className="text-[12px] text-text-secondary hover:text-text-base transition-colors">{t("settings.env.variables")}</button>
              </div>
              <textarea 
                className="w-full h-[120px] p-4 text-[13px] font-mono text-text-base resize-none focus:outline-none bg-transparent"
                defaultValue={`cd "$CODEX_WORKTREE_PATH"\npip install -r requirements.txt\nnpm install\n./run/setup.sh`}
              ></textarea>
            </div>
          </div>

          <div>
            <div className="mb-2">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.env.cleanupScript")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.env.cleanupScriptDesc")}</div>
            </div>
            
            <div className="border border-border-theme rounded-xl overflow-hidden bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
              <div className="flex items-center justify-between px-4 py-2 border-b border-border-theme bg-gray-50/50">
                <div className="flex space-x-4 text-[12px]">
                  {[
                    { value: "default", label: t("settings.env.default") },
                    { value: "macOS", label: "macOS" },
                    { value: "Linux", label: "Linux" },
                    { value: "Windows", label: "Windows" }
                  ].map(tab => (
                    <button 
                      key={tab.value}
                      className={`transition-colors ${cleanupTab === tab.value ? 'font-medium text-text-base' : 'text-text-secondary hover:text-text-base'}`}
                      onClick={() => setCleanupTab(tab.value)}
                    >
                      {tab.label}
                    </button>
                  ))}
                </div>
                <button className="text-[12px] text-text-secondary hover:text-text-base transition-colors">{t("settings.env.variables")}</button>
              </div>
              <textarea 
                className="w-full h-[100px] p-4 text-[13px] font-mono text-text-base resize-none focus:outline-none bg-transparent"
                defaultValue={`docker compose down --remove-orphans\nrm -rf .cache/tmp`}
              ></textarea>
            </div>
          </div>

          <div>
            <div className="flex items-center justify-between mb-2">
              <div>
                <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.env.actions")}</div>
                <div className="text-[12px] text-text-secondary">{t("settings.env.actionsDesc")}</div>
              </div>
              <button className="px-3 py-1.5 bg-black/5 hover:bg-black/5 rounded-full text-[12px] text-text-base font-medium transition-colors">
                {t("settings.env.addAction")}
              </button>
            </div>
            <div className="border border-border-theme rounded-xl p-4 bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
              <div className="text-[13px] text-text-secondary">{t("settings.env.addActionDesc")}</div>
            </div>
          </div>

          <div className="flex justify-end pt-4">
            <button className="px-6 py-1.5 rounded-full text-[13px] font-medium bg-gray-400 text-white cursor-not-allowed">
              {t("settings.env.save")}
            </button>
          </div>
        </div>
      </div>
    );
  }

  // --- List View ---
  return (
    <>
      <div className="mb-10 max-w-[700px]">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.env.title")}</h1>
        <div className="text-[13px] text-text-secondary">
          {t("settings.env.desc1")} <a href="#" className="text-blue-500 hover:underline">{t("settings.env.learnMore")}</a>
        </div>
      </div>

      <div className="max-w-[700px]">
        <div className="flex items-center justify-between mb-4">
          <div className="text-[14px] font-medium text-text-base">{t("settings.env.selectProject")}</div>
          <button className="px-3 py-1.5 bg-black/5 hover:bg-black/5 rounded-full text-[12px] font-medium text-text-base transition-colors">
            {t("settings.env.addProject")}
          </button>
        </div>

        <div className="space-y-3">
          {projects.map((proj, idx) => (
            <div 
              key={idx} 
              className="rounded-xl p-3 flex items-center justify-between bg-black/5 shadow-[0_1px_2px_rgb(0,0,0,0.02)] hover:bg-black/5 transition-colors cursor-pointer"
              onClick={() => {
                if (idx === 0) setView("detail"); // Only wire up the first one for demo
              }}
            >
              <div className="flex items-center">
                <FontAwesomeIcon icon={["far", "folder"]} className="text-gray-400 mr-3 text-[14px]" />
                <span className="text-[13px] font-medium text-text-base mr-2">{proj.name}</span>
                {proj.sub && <span className="text-[12px] text-text-secondary">{proj.sub}</span>}
              </div>
              <button 
                className="w-6 h-6 rounded-md border border-gray-200 flex items-center justify-center text-gray-500 hover:bg-black/5 transition-colors"
                onClick={(e) => {
                  e.stopPropagation();
                  setView("detail");
                }}
              >
                <FontAwesomeIcon icon={["fas", "plus"]} className="text-[10px]" />
              </button>
            </div>
          ))}
        </div>
      </div>
    </>
  );
}
