import { useState } from "react";
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

export function GitSettings() {
  const { t } = useTranslation();
  const [prIcon, setPrIcon] = useState(false);
  const [forcePush, setForcePush] = useState(false);
  const [draftPr, setDraftPr] = useState(true);
  const [deleteWorktrees, setDeleteWorktrees] = useState(true);
  const [mergeMethod, setMergeMethod] = useState<"merge" | "squash">("merge");
  
  const [commitInstructions, setCommitInstructions] = useState("");
  const [prInstructions, setPrInstructions] = useState("");

  return (
    <>
      <div className="mb-10 max-w-[700px]">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.git.title")}</h1>
      </div>

      <div className="mb-12 max-w-[700px]">
        <div className="border border-border-theme rounded-xl overflow-hidden bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          {/* 分支前缀 */}
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.branchPrefix")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.git.branchPrefixDesc")}</div>
            </div>
            <input 
              type="text" 
              defaultValue="codex/"
              className="w-[200px] border border-border-theme rounded-lg py-1.5 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white shadow-sm"
            />
          </div>

          {/* 拉取请求合并方法 */}
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.prMergeMethod")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.git.prMergeMethodDesc")}</div>
            </div>
            <div className="flex border border-border-theme rounded-full overflow-hidden bg-gray-50 p-0.5">
              <button 
                className={`px-4 py-1.5 text-[12px] font-medium rounded-full transition-colors ${mergeMethod === 'merge' ? 'bg-white text-text-base shadow-sm border border-gray-200' : 'text-text-secondary hover:text-text-base border border-transparent'}`}
                onClick={() => setMergeMethod("merge")}
              >
                {t("settings.git.merge")}
              </button>
              <button 
                className={`px-4 py-1.5 text-[12px] font-medium rounded-full transition-colors ${mergeMethod === 'squash' ? 'bg-white text-text-base shadow-sm border border-gray-200' : 'text-text-secondary hover:text-text-base border border-transparent'}`}
                onClick={() => setMergeMethod("squash")}
              >
                {t("settings.git.squash")}
              </button>
            </div>
          </div>

          {/* 在侧边栏显示 PR 图标 */}
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.showPrIcon")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.git.showPrIconDesc")}</div>
            </div>
            <ToggleSwitch checked={prIcon} onChange={() => setPrIcon(!prIcon)} />
          </div>

          {/* 始终强制推送 */}
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.alwaysForcePush")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.git.forcePushDesc")}</div>
            </div>
            <ToggleSwitch checked={forcePush} onChange={() => setForcePush(!forcePush)} />
          </div>

          {/* 创建草稿拉取请求 */}
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.draftPr")}</div>
              <div className="text-[12px] text-text-secondary">{t("settings.git.draftPrDesc")}</div>
            </div>
            <ToggleSwitch checked={draftPr} onChange={() => setDraftPr(!draftPr)} />
          </div>

          {/* 自动删除旧工作树 */}
          <div className="flex items-center justify-between p-4 border-b border-border-theme">
            <div className="pr-4">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.autoDelete")}</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                {t("settings.git.autoDeleteDesc")}
              </div>
            </div>
            <ToggleSwitch checked={deleteWorktrees} onChange={() => setDeleteWorktrees(!deleteWorktrees)} />
          </div>

          {/* 自动删除限制 */}
          <div className="flex items-center justify-between p-4">
            <div className="pr-4 max-w-[500px]">
              <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.autoDeleteLimit")}</div>
              <div className="text-[12px] text-text-secondary leading-relaxed">
                {t("settings.git.autoDeleteLimitDesc")}
              </div>
            </div>
            <input 
              type="number" 
              defaultValue="15"
              className="w-[80px] border border-border-theme rounded-lg py-1.5 px-3 text-[13px] focus:outline-none focus:border-blue-500 bg-white shadow-sm"
            />
          </div>
        </div>
      </div>

      {/* 提交指令 */}
      <div className="mb-10 max-w-[700px]">
        <div className="flex items-center justify-between mb-3">
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.commitInstructions")}</div>
            <div className="text-[12px] text-text-secondary">{t("settings.git.commitInstructionsDesc")}</div>
          </div>
          <button 
            className={`px-4 py-1.5 rounded-full text-[13px] font-medium transition-colors ${commitInstructions.trim() !== "" ? 'bg-blue-500 text-white hover:bg-blue-600 shadow-sm' : 'bg-gray-100 text-gray-400 cursor-not-allowed'}`}
            disabled={commitInstructions.trim() === ""}
          >
            {t("settings.git.save")}
          </button>
        </div>
        <div className="border border-border-theme rounded-xl overflow-hidden bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <textarea 
            className="w-full h-[120px] p-4 text-[13px] text-text-base resize-none focus:outline-none placeholder-gray-400 bg-transparent"
            placeholder={t("settings.git.commitInstructionsPlaceholder")}
            value={commitInstructions}
            onChange={(e) => setCommitInstructions(e.target.value)}
          ></textarea>
        </div>
      </div>

      {/* 拉取请求指令 */}
      <div className="mb-20 max-w-[700px]">
        <div className="flex items-center justify-between mb-3">
          <div>
            <div className="text-[14px] font-medium text-text-base mb-1">{t("settings.git.prInstructions")}</div>
            <div className="text-[12px] text-text-secondary">{t("settings.git.prInstructionsDesc")}</div>
          </div>
          <button 
            className={`px-4 py-1.5 rounded-full text-[13px] font-medium transition-colors ${prInstructions.trim() !== "" ? 'bg-blue-500 text-white hover:bg-blue-600 shadow-sm' : 'bg-gray-100 text-gray-400 cursor-not-allowed'}`}
            disabled={prInstructions.trim() === ""}
          >
            {t("settings.git.save")}
          </button>
        </div>
        <div className="border border-border-theme rounded-xl overflow-hidden bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
          <textarea 
            className="w-full h-[120px] p-4 text-[13px] text-text-base resize-none focus:outline-none placeholder-gray-400 bg-transparent"
            placeholder={t("settings.git.prInstructionsPlaceholder")}
            value={prInstructions}
            onChange={(e) => setPrInstructions(e.target.value)}
          ></textarea>
        </div>
      </div>
    </>
  );
}
