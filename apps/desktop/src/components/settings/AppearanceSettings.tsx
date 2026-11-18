import { useTranslation } from "react-i18next";
import { AppearanceThemeSection } from "./AppearanceThemeSection";
import { AppearanceBackgroundSection } from "./AppearanceBackgroundSection";

export function AppearanceSettings() {
  const { t } = useTranslation();

  return (
    <>
      <h1 className="text-2xl font-semibold text-text-base mb-10">{t("settings.appearance.title")}</h1>
      <AppearanceBackgroundSection />
      <AppearanceThemeSection />
    </>
  );
}
