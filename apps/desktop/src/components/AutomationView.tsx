import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";

export function AutomationView() {
  const { t } = useTranslation();
  return (
    <div className="w-full h-full flex flex-col bg-white overflow-hidden relative">
      
      {/* Top right actions */}
      <div className="absolute top-6 right-8 flex items-center space-x-3">
        <button className="flex items-center text-text-base bg-gray-50 border border-border-theme hover:bg-gray-100 rounded-lg px-4 py-1.5 text-[13px] font-medium transition-colors">
          {t("automationView.viewTemplates")}
        </button>
        <button className="flex items-center text-white bg-text-base hover:bg-black rounded-lg px-4 py-1.5 text-[13px] font-medium transition-colors">
          {t("automationView.createViaChat")}
          <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-2 text-[10px]" />
        </button>
      </div>

      {/* Main Content Centered */}
      <div className="flex-1 flex flex-col items-center pt-[10vh] px-12">
        {/* Header */}
        <div className="text-center mb-24">
          <h1 className="text-3xl font-semibold text-text-base mb-2">{t("automationView.title")}</h1>
          <p className="text-[13px] text-text-secondary">
            {t("automationView.subtitle")} <a href="#" className="text-blue-500 hover:underline">{t("automationView.learnMore")}</a>
          </p>
        </div>

        {/* Empty State Center */}
        <div className="flex flex-col items-center">
          <div className="w-20 h-20 rounded-full border-[3px] border-text-base flex items-center justify-center mb-8">
            <FontAwesomeIcon icon={["far", "clock"]} className="text-4xl text-text-base" />
          </div>
          
          <h2 className="text-base font-medium text-text-base mb-6">{t("automationView.createFirst")}</h2>
          
          <div className="flex items-center space-x-3">
            <button className="flex items-center text-[13px] text-text-secondary bg-white border border-border-theme rounded-lg px-4 py-2 hover:bg-gray-50 hover:text-text-base transition-colors shadow-sm">
              <FontAwesomeIcon icon={["far", "bell"]} className="mr-2" />
              {t("automationView.dailyBriefing")}
            </button>
            <button className="flex items-center text-[13px] text-text-secondary bg-white border border-border-theme rounded-lg px-4 py-2 hover:bg-gray-50 hover:text-text-base transition-colors shadow-sm">
              <FontAwesomeIcon icon={["far", "calendar-check"]} className="mr-2" />
              {t("automationView.weeklyReview")}
            </button>
            <button className="flex items-center text-[13px] text-text-secondary bg-white border border-border-theme rounded-lg px-4 py-2 hover:bg-gray-50 hover:text-text-base transition-colors shadow-sm">
              <FontAwesomeIcon icon={["fas", "magnifying-glass-chart"]} className="mr-2" />
              {t("automationView.projectMonitoring")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
