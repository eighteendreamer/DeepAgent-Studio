import { HoverInfo } from "./ui/HoverInfo";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, CircleHelp, type LucideIcon } from "lucide-react";
import type { ThinkingLevel, ThinkingSelection } from "../hooks/useComposerModelSettings";

import { EffortSlider } from "./ui/EffortSlider";

import { ListItem } from "./ui/ListItem";

import { TintButton } from "./ui/TintButton";

import { MorphingMenuShell } from "./ui/MorphingMenuShell";

import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";

import { cn } from "./shadcn/utils";



type ReasoningEffort = ThinkingLevel;



type ThinkingOption = {

  id: ReasoningEffort;

  label: string;

  icon: LucideIcon;

};



export type ModelOption = {
  id: string;
  label: string;
  providerLabel: string;
  thinkingLevels?: string[];
  thinkingDistinct?: string[];
  thinkingDisable?: boolean;
  defaultThinking?: ThinkingSelection;
};

type Props = {

  open: boolean;

  disabled?: boolean;

  switching?: boolean;

  models: readonly ModelOption[];

  selectedModel: string;

  selectedThinking: ThinkingSelection;

  thinkingOptions: readonly ThinkingOption[];

  thinkingLevels?: readonly string[];

  thinkingDisable?: boolean;

  selectModelLabel: string;

  noModelsLabel: string;

  onOpenChange: (open: boolean) => void;

  onChooseModel: (id: string) => void;

  onChooseThinking: (id: ThinkingLevel) => void;

  triggerClassName?: string;

};



/** 标题、选项容器与滑轨统一外沿，选项文字保留独立内边距。 */
const MODEL_MENU = {
  padX: "px-3",
  divider: "mx-3 my-2 h-px shrink-0 bg-border-theme opacity-[0.55]",
  section: "flex items-center py-1 text-[12px]",
  pill: "left-0 right-0 rounded-lg",
} as const;



export function ModelThinkingSelector({

  open,

  disabled = false,

  switching = false,

  models,

  selectedModel,

  selectedThinking,

  thinkingOptions,
  thinkingLevels,
  thinkingDisable,

  selectModelLabel,

  noModelsLabel,

  onOpenChange,

  onChooseModel,

  onChooseThinking,

  triggerClassName,

}: Props) {
  const { t } = useTranslation();

  const morphLayoutId = useId().replace(/:/g, "");

  const unavailableOption = { label: t("composer.thinkingUnavailable"), icon: CircleHelp };
  const selectedThinkingOption = thinkingOptions.find(option => option.id === selectedThinking) ?? unavailableOption;

  const selectedOption = models.find((model) => model.id === selectedModel);
  const declaredLevels = selectedOption?.thinkingDistinct?.length ? selectedOption.thinkingDistinct : thinkingLevels ?? selectedOption?.thinkingLevels ?? [];
  const supportedLevels = new Set(declaredLevels.map((level) => level === "none" ? "off" : level));
  const canDisableThinking = thinkingDisable ?? selectedOption?.thinkingDisable ?? true;
  const effortOptions = thinkingOptions.filter(option => supportedLevels.has(option.id)
    && (option.id !== "off" || canDisableThinking));
  const pillModel = selectedOption?.label || (selectedModel ? selectedModel : selectModelLabel);

  const trigger = (
    <HoverInfo content={`${selectModelLabel} / ${selectedThinkingOption.label}`}><TintButton
      type="button"
      disabled={disabled}
      aria-expanded={open}
      onClick={() => onOpenChange(!open)}
      className={cn("flex h-8 max-w-[210px] flex-shrink-0 items-center rounded-full px-3 text-xs", triggerClassName)}

    >
      <selectedThinkingOption.icon size={12} className="mr-1.5 text-text-secondary" />
      <span className="font-medium">{pillModel}</span>
      <span className="ml-1.5 shrink-0 text-text-secondary">{selectedThinkingOption.label}</span>
      <ChevronDown size={10} className="ml-2 text-text-secondary" />
    </TintButton></HoverInfo>
  );

  return (
    <MorphingMenuShell
      open={open}
      onOpenChange={onOpenChange}
      layoutId={morphLayoutId}
      trigger={trigger}
      className="flex h-8 min-w-0 max-w-full items-center"
      panelClassName="w-[252px]"
      panelAlign="right"
      zIndex={50}
    >
          <div className="py-2">

          <div className={cn(MODEL_MENU.padX, "pb-1")}>

            <div className={MODEL_MENU.section}>

              <span className="font-medium text-text-base">模型</span>


            </div>



            <div className="max-h-[240px] overflow-y-auto overscroll-contain">

              {models.length === 0 ? (

                <div className="rounded-lg px-2.5 py-2 text-[12px] text-text-secondary">{noModelsLabel}</div>

              ) : (

                <SlidingMenuList activeId={selectedModel} pillClassName={MODEL_MENU.pill} className="w-full py-0.5">

                  {Array.from(new Map(models.map((model) => [model.providerLabel, model])).keys()).map((provider) => (
                    <div key={provider} className="pb-1 last:pb-0">
                      <div className="flex items-center gap-2 px-2.5 pb-1 pt-2 text-[11px] font-normal text-text-secondary">
                        <span className="min-w-0 truncate">{provider}</span>
                        <span aria-hidden="true" className="h-px min-w-3 flex-1 bg-border-theme opacity-[0.55]" />
                      </div>
                      {models.filter((model) => model.providerLabel === provider).map((model) => {
                        const selected = model.id === selectedModel;
                        return (
                          <ListItem
                            key={model.id}
                            {...{ [MENU_ITEM_ATTR]: model.id }}
                            sliding
                            selected={selected}
                            aria-busy={switching || undefined}
                            onClick={() => onChooseModel(model.id)}
                            className="cursor-pointer gap-2 py-2 pl-5 pr-2.5 text-left text-[13px] text-text-base"
                          >
                            <span className={cn("min-w-0 truncate", selected ? "font-semibold" : "font-medium")}>{model.label}</span>
                            {selected && <Check size={12} className="shrink-0 text-text-base" />}
                          </ListItem>
                        );
                      })}
                    </div>
                  ))}

                </SlidingMenuList>

              )}

            </div>

          </div>



          <div className={MODEL_MENU.divider} />



          <div className="px-3 pb-1 pt-0.5">

            <div className="mb-2.5 flex items-center text-[12px]">

              <span className="font-medium text-text-base">{t("composer.selectThinking")}</span>

            </div>

            {effortOptions.length <= 1 ? <div className="py-1 text-sm text-text-secondary">{effortOptions[0]?.label ?? unavailableOption.label}</div> : <EffortSlider

              stops={effortOptions.map((option) => ({
                value: option.id,
                label: option.label,
                icon: option.icon,
              }))}

              value={selectedThinking ?? ""}

              onChange={(value) => onChooseThinking(value as ThinkingLevel)}

              ariaLabel={t("composer.selectThinking")}
              disabled={disabled}

            />}

          </div>

          </div>
    </MorphingMenuShell>
  );

}

