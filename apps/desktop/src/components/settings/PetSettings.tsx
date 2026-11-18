import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { Button } from "../shadcn/button";

function PetItem({ name, desc, icon, iconColor, selected }: { name: string, desc: string, icon: IconProp, iconColor: string, selected: boolean }) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center justify-between p-3 rounded-xl transition-colors cursor-pointer hover:bg-black/5">
      <div className="flex items-center">
        <div className="w-12 h-12 rounded-lg bg-black/5 flex items-center justify-center mr-4 shadow-sm">
          <FontAwesomeIcon icon={icon} className={`text-xl ${iconColor}`} />
        </div>
        <div>
          <div className="text-[13px] font-medium text-text-base mb-0.5">{name}</div>
          <div className="text-[12px] text-text-secondary">{desc}</div>
        </div>
      </div>
      <div>
        {selected ? (
          <div className="px-4 py-1.5 bg-black/5 text-gray-400 rounded-md text-[12px] font-medium">
            {t("settings.appearance.selected")}
          </div>
        ) : (
          <div className="px-4 py-1.5 bg-black/5 text-text-base hover:bg-black/5 rounded-md text-[12px] font-medium transition-colors">
            {t("settings.appearance.select")}
          </div>
        )}
      </div>
    </div>
  );
}

export function PetSettings() {
  const { t } = useTranslation();
  const [isPetExpanded, setIsPetExpanded] = useState(true);

  return (
    <>
      <h1 className="text-2xl font-semibold text-text-base mb-10">{t("settings.tabs.pet")}</h1>

      <div className="mb-6 max-w-[700px]">
        <div className="border border-border-theme rounded-xl shadow-[0_1px_2px_rgb(0,0,0,0.02)] bg-white overflow-hidden">
          <Button
            variant="ghost"
            className="h-auto w-full justify-between rounded-none p-4 text-left hover:bg-black/5 border-b border-border-theme"
            aria-expanded={isPetExpanded}
            aria-controls="pet-settings-content"
            onClick={() => setIsPetExpanded(expanded => !expanded)}
          >
            <span>
              <span className="block text-[15px] font-medium text-text-base mb-1">{t("settings.appearance.pet")}</span>
              <span className="block text-[13px] font-normal text-text-secondary">{t("settings.appearance.selectedPet", { pet: "Codex" })}</span>
            </span>
            <FontAwesomeIcon icon={["fas", isPetExpanded ? "chevron-up" : "chevron-down"]} className="text-[12px] text-text-secondary" />
          </Button>

          <div id="pet-settings-content" hidden={!isPetExpanded}>
            <div className="p-4 bg-gray-50/50 border-b border-border-theme flex justify-end space-x-2">
              <Button variant="secondary" size="sm" className="bg-black/5 rounded-md shadow-sm">
                {t("settings.appearance.createPet")}
              </Button>
              <Button variant="secondary" size="sm" className="bg-black/5 rounded-md shadow-sm">
                {t("settings.appearance.refresh")}
              </Button>
              <Button variant="secondary" size="sm" className="bg-black/5 rounded-md shadow-sm">
                {t("settings.appearance.wakePet")}
              </Button>
            </div>

            <div className="p-4 bg-white space-y-3">
              <PetItem
                name="Codex"
                desc="The original Codex companion."
                icon={["fas", "robot"]}
                iconColor="text-blue-500"
                selected={true}
              />
              <PetItem
                name="Dewey"
                desc="A tidy duck for calm workspace days."
                icon={["fas", "cloud"]}
                iconColor="text-cyan-500"
                selected={false}
              />
              <PetItem
                name="Fireball"
                desc="Hot path energy for fast iteration."
                icon={["fas", "bullseye"]}
                iconColor="text-orange-500"
                selected={false}
              />
              <PetItem
                name="Rocky"
                desc="A steady rock when the diff gets large."
                icon={["fas", "cube"]}
                iconColor="text-stone-500"
                selected={false}
              />
              <PetItem
                name="Seedy"
                desc="Small green shoots for new ideas."
                icon={["fas", "leaf"]}
                iconColor="text-green-500"
                selected={false}
              />
              <PetItem
                name="Stacky"
                desc="A balanced stack for deep work."
                icon={["fas", "layer-group"]}
                iconColor="text-purple-500"
                selected={false}
              />
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
