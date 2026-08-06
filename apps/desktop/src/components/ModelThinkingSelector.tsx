import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Slider } from "./ui/Slider";
import { Panel } from "./ui/Panel";
import { ListItem } from "./ui/ListItem";
import { TintButton } from "./ui/TintButton";

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
}: Props) {
  const selectedThinkingOption =
    thinkingOptions.find((option) => option.id === selectedThinking) ?? thinkingOptions[1];
  const pillModel = selectedModel ? compactModelLabel(selectedModel) : selectModelLabel;

  return (
    <div className="relative">
      <TintButton
        type="button"
        disabled={disabled}
        onClick={() => onOpenChange(!open)}
        className="flex h-9 max-w-[210px] flex-shrink-0 items-center rounded-[10px] px-3 text-xs"
        title={`${selectModelLabel} / ${selectedThinkingOption.label}`}
      >
        <FontAwesomeIcon icon={selectedThinkingOption.icon as any} className="mr-1.5 text-[11px] text-text-secondary" />
        <span className="truncate font-medium">{pillModel}</span>
        <span className="ml-1.5 shrink-0 text-text-secondary">{selectedThinkingOption.label}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-2 text-[9px] text-text-secondary" />
      </TintButton>

      {open && (
        <Panel className="absolute bottom-full right-0 z-50 mb-2 w-[264px] overflow-hidden py-2">
          <div className="px-2 pb-0.5">
            <div className="flex items-center justify-between px-1 py-1 text-[12px]">
              <span className="font-medium text-text-base">模型</span>
              <span className="max-w-[150px] truncate text-text-secondary">{modelLabel(selectedModel) || "未选择"}</span>
            </div>

            <div className="max-h-[96px] overflow-y-auto py-0.5">
              {models.length === 0 ? (
                <div className="rounded-md px-2 py-1.5 text-[12px] text-text-secondary">{noModelsLabel}</div>
              ) : (
                models.map((id) => {
                  const selected = id === selectedModel;
                  return (
                    <ListItem
                      key={id}
                      selected={selected}
                      onClick={switching ? undefined : () => onChooseModel(id)}
                      className={`text-left cursor-pointer ${
                        selected ? "text-text-base" : "text-text-secondary hover:text-text-base"
                      }`}
                    >
                      <span className="truncate font-medium">{modelLabel(id)}</span>
                      {selected && <FontAwesomeIcon icon={["fas", "check"]} className="ml-3 text-[10px] text-text-base" />}
                    </ListItem>
                  );
                })
              )}
            </div>
          </div>

          <div className="mx-2 my-1.5 h-px bg-border-theme" />

          <div className="px-2 pb-1 pt-0.5">
            <div className="mb-1.5 flex items-center justify-between px-1 text-[12px]">
              <span className="font-medium text-text-base">推理强度</span>
            </div>
            <Slider
              stops={thinkingOptions.map((option) => ({ value: option.id, label: option.label }))}
              value={selectedThinking}
              onChange={(value) => onChooseThinking(value as ThinkingDepth)}
              ariaLabel="推理强度"
            />
          </div>
        </Panel>
      )}
    </div>
  );
}
