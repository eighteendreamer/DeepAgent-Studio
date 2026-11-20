import { memo, Suspense, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { preloadSettingsCategory, settingsCategories, settingsCategory } from "./settings/settingsCategories";
import { SurfaceVisibility } from "./ui/SurfaceVisibility";

interface Props {
  activeCategoryId: string;
  visible?: boolean;
}

export const SettingsView = memo(function SettingsView({ activeCategoryId, visible = true }: Props) {
  const { t } = useTranslation();
  const active = settingsCategory(activeCategoryId);
  const [visited, setVisited] = useState([active]);
  const categories = visited.includes(active) ? visited : [...visited, active];
  useEffect(() => {
    setVisited(current => current.includes(active) ? current : [...current, active]);
  }, [active]);
  useEffect(() => {
    // Warm code only, one category at a time. Unvisited pages do not read data.
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const remaining = Object.keys(settingsCategories);
    const next = () => {
      const id = remaining.shift();
      if (cancelled || !id) return;
      void preloadSettingsCategory(id).catch(() => {}).finally(() => {
        if (!cancelled) timer = setTimeout(next, 100);
      });
    };
    timer = setTimeout(next, 500);
    return () => { cancelled = true; clearTimeout(timer); };
  }, []);

  return (
    <div data-settings-viewport className={`h-full min-h-0 w-full bg-bg-base text-text-base max-[900px]:px-6 ${active === "models" ? "overflow-hidden px-10 py-6" : "stable-scrollbar-gutter overflow-y-auto px-10 pb-16 pt-10"}`}>
      <div className={`mx-auto w-full ${active === "models" ? "h-full min-h-0 max-w-[1040px]" : active === "usage" ? "max-w-[1120px]" : active === "resources" ? "max-w-[900px]" : "max-w-[700px]"}`}>
        {categories.map(id => {
          const Category = settingsCategories[id];
          return <section key={id} hidden={id !== active} className={`settings-section w-full ${id === "models" ? "h-full min-h-0" : ""}`} data-settings-category={id}>
            <SurfaceVisibility.Provider value={visible && id === active}>
              <Suspense fallback={<h1 data-category-pending className="text-2xl font-semibold">{t(`settings.tabs.${id}`)}</h1>}>
                <Category />
              </Suspense>
            </SurfaceVisibility.Provider>
          </section>;
        })}
      </div>
    </div>
  );
});
