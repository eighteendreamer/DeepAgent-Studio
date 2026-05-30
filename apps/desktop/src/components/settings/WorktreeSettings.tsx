import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";

export function WorktreeSettings() {
  const { t } = useTranslation();
  return (
    <>
      <div className="mb-10 max-w-[700px]">
        <h1 className="text-2xl font-semibold text-text-base mb-1">{t("settings.worktrees.title")}</h1>
      </div>

      <div className="max-w-[700px]">
        <div className="flex items-center justify-between mb-2">
          <div className="text-[14px] font-medium text-text-base">{t("settings.worktrees.noWorktrees")}</div>
          <button className="text-gray-400 hover:text-text-base transition-colors">
            <FontAwesomeIcon icon={["fas", "sync-alt"]} className="text-[12px]" />
          </button>
        </div>

        <div className="border border-border-theme rounded-xl bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)] p-4">
          <div className="text-[13px] text-text-secondary">{t("settings.worktrees.noWorktreesDesc")}</div>
        </div>
      </div>
    </>
  );
}
