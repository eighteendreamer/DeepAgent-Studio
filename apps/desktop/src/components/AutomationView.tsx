import {
  Bell,
  CalendarCheck2,
  ChartNoAxesCombined,
  ChevronDown,
  Clock3,
  LayoutTemplate,
  MessageSquarePlus,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "./shadcn/button";

export function AutomationView() {
  const { t } = useTranslation();
  return (
    <div className="w-full h-full flex flex-col bg-white overflow-hidden relative">
      
      {/* Top right actions */}
      <div className="absolute top-6 right-8 flex items-center space-x-3">
        <Button
          variant="ghost"
          className="h-auto rounded-lg bg-black/5 px-4 py-1.5 text-[13px] font-medium text-text-base hover:bg-black/10"
        >
          <LayoutTemplate className="h-3.5 w-3.5" aria-hidden="true" />
          {t("automationView.viewTemplates")}
        </Button>
        <Button
          className="h-auto rounded-lg bg-text-base px-4 py-1.5 text-[13px] font-medium text-white hover:bg-black"
        >
          <MessageSquarePlus className="h-3.5 w-3.5" aria-hidden="true" />
          {t("automationView.createViaChat")}
          <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
        </Button>
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
            <Clock3 className="h-10 w-10 text-text-base" aria-hidden="true" />
          </div>
          
          <h2 className="text-base font-medium text-text-base mb-6">{t("automationView.createFirst")}</h2>
          
          <div className="flex items-center space-x-3">
            <Button
              variant="ghost"
              className="h-auto rounded-lg bg-black/5 px-4 py-2 text-[13px] text-text-secondary shadow-sm hover:bg-black/10 hover:text-text-base"
            >
              <Bell className="h-3.5 w-3.5" aria-hidden="true" />
              {t("automationView.dailyBriefing")}
            </Button>
            <Button
              variant="ghost"
              className="h-auto rounded-lg bg-black/5 px-4 py-2 text-[13px] text-text-secondary shadow-sm hover:bg-black/10 hover:text-text-base"
            >
              <CalendarCheck2 className="h-3.5 w-3.5" aria-hidden="true" />
              {t("automationView.weeklyReview")}
            </Button>
            <Button
              variant="ghost"
              className="h-auto rounded-lg bg-black/5 px-4 py-2 text-[13px] text-text-secondary shadow-sm hover:bg-black/10 hover:text-text-base"
            >
              <ChartNoAxesCombined className="h-3.5 w-3.5" aria-hidden="true" />
              {t("automationView.projectMonitoring")}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
