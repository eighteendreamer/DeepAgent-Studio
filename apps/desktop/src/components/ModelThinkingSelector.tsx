import { useId } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

import { Slider } from "./ui/Slider";

import { ListItem } from "./ui/ListItem";

import { TintButton } from "./ui/TintButton";

import { MorphingMenuShell } from "./ui/MorphingMenuShell";

import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";

import { cn } from "./shadcn/utils";



type ThinkingDepth = "simple" | "medium" | "deep";



type ThinkingOption = {

  id: ThinkingDepth;

  label: string;

  icon: readonly unknown[];

};



type Props = {

  open: boolean;

  disabled?: boolean;

  switching?: boolean;

  models: string[];

  selectedModel: string;

  selectedThinking: ThinkingDepth;

  thinkingOptions: readonly ThinkingOption[];

  selectModelLabel: string;

  noModelsLabel: string;

  onOpenChange: (open: boolean) => void;

  onChooseModel: (id: string) => void;

  onChooseThinking: (id: ThinkingDepth) => void;

  triggerClassName?: string;

};



function modelLabel(id: string): string {

  if (!id) return "";

  if (id.startsWith("deepseek-")) return id.slice("deepseek-".length);

  return id;

}



function compactModelLabel(id: string): string {

  const label = modelLabel(id);

  if (label.startsWith("v4-")) return label;

  return label.length > 18 ? `${label.slice(0, 17)}…` : label;

}

/** 模型/推理浮层 —— px-2 外框 + 药丸贴齐内容区 */
const MODEL_MENU = {
  padX: "px-2",
  divider: "mx-2 my-1.5 h-px shrink-0 bg-border-theme opacity-[0.55]",
  section: "flex items-center justify-between px-0.5 py-1 text-[12px]",
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

  selectModelLabel,

  noModelsLabel,

  onOpenChange,

  onChooseModel,

  onChooseThinking,

  triggerClassName,

}: Props) {

  const morphLayoutId = useId().replace(/:/g, "");

  const selectedThinkingOption =

    thinkingOptions.find((option) => option.id === selectedThinking) ?? thinkingOptions[1];

  const pillModel = selectedModel ? compactModelLabel(selectedModel) : selectModelLabel;

  const trigger = (
    <TintButton
      type="button"
      disabled={disabled}
      onClick={() => onOpenChange(!open)}
      className={cn("flex h-8 max-w-[210px] flex-shrink-0 items-center rounded-full px-3 text-xs", triggerClassName)}
      title={`${selectModelLabel} / ${selectedThinkingOption.label}`}
    >
      <FontAwesomeIcon icon={selectedThinkingOption.icon as any} className="mr-1.5 text-[11px] text-text-secondary" />
      <span className="truncate font-medium">{pillModel}</span>
      <span className="ml-1.5 shrink-0 text-text-secondary">{selectedThinkingOption.label}</span>
      <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-2 text-[9px] text-text-secondary" />
    </TintButton>
  );

  return (
    <MorphingMenuShell
      open={open}
      onOpenChange={onOpenChange}
      layoutId={morphLayoutId}
      trigger={trigger}
      className="flex h-8 min-w-0 max-w-full items-center"
      panelClassName="w-[264px]"
      panelAlign="right"
      zIndex={50}
    >
          <div className="py-2">

          <div className={cn(MODEL_MENU.padX, "pb-1")}>

            <div className={MODEL_MENU.section}>

              <span className="font-medium text-text-base">模型</span>

              <span className="max-w-[150px] truncate text-text-secondary">{modelLabel(selectedModel) || "未选择"}</span>

            </div>



            <div className="max-h-[96px] overflow-y-auto">

              {models.length === 0 ? (

                <div className="rounded-lg px-2.5 py-2 text-[12px] text-text-secondary">{noModelsLabel}</div>

              ) : (

                <SlidingMenuList activeId={selectedModel} pillClassName={MODEL_MENU.pill} className="w-full py-0.5">

                  {models.map((id) => {

                    const selected = id === selectedModel;

                    return (

                      <ListItem

                        key={id}

                        {...{ [MENU_ITEM_ATTR]: id }}

                        sliding

                        selected={selected}

                        onClick={switching ? undefined : () => onChooseModel(id)}

                        className={cn(
                          "cursor-pointer px-2.5 text-left",
                          selected ? "text-text-base" : "text-text-secondary",
                        )}

                      >

                        <span className="truncate font-medium">{modelLabel(id)}</span>

                        {selected && <FontAwesomeIcon icon={["fas", "check"]} className="ml-3 text-[10px] text-text-base" />}

                      </ListItem>

                    );

                  })}

                </SlidingMenuList>

              )}

            </div>

          </div>



          <div className={MODEL_MENU.divider} />



          <div className={cn(MODEL_MENU.padX, "pt-0.5")}>

            <div className="mb-1.5 flex items-center justify-between px-0.5 text-[12px]">

              <span className="font-medium text-text-base">推理强度</span>

            </div>

            <Slider

              stops={thinkingOptions.map((option) => ({ value: option.id, label: option.label }))}

              value={selectedThinking}

              onChange={(value) => onChooseThinking(value as ThinkingDepth)}

              ariaLabel="推理强度"

            />

          </div>

          </div>
    </MorphingMenuShell>
  );

}

