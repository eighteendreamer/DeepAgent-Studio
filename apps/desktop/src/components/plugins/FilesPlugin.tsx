import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";

export function FilesPlugin() {
  const { t } = useTranslation();
  return (
    <div className="w-full h-full flex flex-col bg-white">
      {/* Content */}
      <div className="flex-1 flex overflow-hidden">
        {/* Left: Editor/Viewer */}
        <div className="flex-1 flex flex-col overflow-y-auto px-8 py-6">
          <div className="text-[13px] text-text-secondary mb-4 flex items-center">
            Looker-v2
            <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] mx-2" />
            AUTH_SPEC.md
          </div>
          
          <h1 className="text-2xl font-bold text-text-base mb-4">{t("plugins.files.specTitle")}</h1>
          <p className="text-[13px] text-text-secondary italic mb-6">
            {t("plugins.files.specDesc")}
          </p>

          <h2 className="text-lg font-bold text-text-base mb-3">{t("plugins.files.corePrinciples")}</h2>
          <ol className="list-decimal pl-5 text-[14px] text-text-base space-y-2 mb-6">
            <li>{t("plugins.files.p1")}</li>
            <li>{t("plugins.files.p2")}</li>
            <li>{t("plugins.files.p3")}</li>
          </ol>

          <h2 className="text-lg font-bold text-text-base mb-3">{t("plugins.files.tokenSpec")}</h2>
          <table className="w-full text-left border-collapse text-[14px]">
            <thead>
              <tr className="border-b border-border-theme">
                <th className="py-2 font-medium text-text-secondary">{t("plugins.files.field")}</th>
                <th className="py-2 font-medium text-text-secondary">{t("plugins.files.value")}</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td className="py-2 text-text-base">{t("plugins.files.format")}</td>
                <td className="py-2 text-text-base">JWT (RS256 / HS256)</td>
              </tr>
            </tbody>
          </table>
        </div>

        {/* Right: File Tree */}
        <div className="w-64 border-l border-border-theme flex flex-col bg-white flex-shrink-0">
          <div className="p-3">
            <div className="relative">
              <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-secondary text-[13px]" />
              <input 
                type="text" 
                placeholder={t("plugins.files.filterPlaceholder")} 
                className="w-full bg-gray-50 border border-border-theme rounded px-8 py-1.5 text-[13px] outline-none focus:border-gray-300"
              />
            </div>
          </div>
          <div className="flex-1 overflow-y-auto px-2 pb-4 text-[13px] text-text-base">
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer">
              <FontAwesomeIcon icon={["fas", "chevron-right"]} className="w-4 text-text-secondary text-[10px]" />
              <span className="ml-1">scripts</span>
            </div>
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer">
              <FontAwesomeIcon icon={["fas", "chevron-right"]} className="w-4 text-text-secondary text-[10px]" />
              <span className="ml-1">tmp</span>
            </div>
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer">
              <FontAwesomeIcon icon={["fas", "chevron-right"]} className="w-4 text-text-secondary text-[10px]" />
              <span className="ml-1">xhc</span>
            </div>
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer">
              <FontAwesomeIcon icon={["fas", "chevron-right"]} className="w-4 text-text-secondary text-[10px]" />
              <span className="ml-1">xhc-front-end</span>
            </div>
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer">
              <FontAwesomeIcon icon={["fas", "chevron-right"]} className="w-4 text-text-secondary text-[10px]" />
              <span className="ml-1">zhongyou-entry</span>
            </div>
            <div className="flex items-center py-1.5 px-2 bg-gray-100 rounded cursor-pointer text-green-600">
              <span className="w-4 font-bold text-[10px] text-center font-mono">M↓</span>
              <span className="ml-1 text-text-base font-medium">AUTH_SPEC.md</span>
            </div>
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer text-green-600">
              <span className="w-4 font-bold text-[10px] text-center font-mono">M↓</span>
              <span className="ml-1 text-text-base">BILLING_SPEC.md</span>
            </div>
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer text-text-secondary">
              <FontAwesomeIcon icon={["far", "file-lines"]} className="w-4" />
              <span className="ml-1 text-text-base">deploy.ps1</span>
            </div>
            <div className="flex items-center py-1.5 px-2 hover:bg-gray-100 rounded cursor-pointer text-green-600">
              <span className="w-4 font-bold text-[10px] text-center font-mono">M↓</span>
              <span className="ml-1 text-text-base">landing-page-solution.md</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
