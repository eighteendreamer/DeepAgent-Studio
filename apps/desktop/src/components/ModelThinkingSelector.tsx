import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";

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
      <button
        type="button"
        disabled={disabled}
        onClick={() => onOpenChange(!open)}
        className="flex h-8 max-w-[160px] flex-shrink-0 items-center rounded-full border border-border-theme bg-gray-50 px-2.5 text-xs text-text-base transition-colors hover:bg-gray-100 disabled:cursor-not-allowed disabled:opacity-50"
        title={`${selectModelLabel} / ${selectedThinkingOption.label}`}
      >
        <FontAwesomeIcon icon={selectedThinkingOption.icon as any} className="mr-1.5 text-text-secondary" />
        <span className="truncate font-medium">{pillModel}</span>
        <span className="ml-1.5 shrink-0 text-text-secondary">{selectedThinkingOption.label}</span>
        <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-2 text-[10px] text-text-secondary" />
      </button>

      {open && (
        <div className="absolute bottom-full right-0 z-50 mb-2 w-[168px] overflow-hidden rounded-xl border border-border-theme bg-white py-1.5 shadow-[0_10px_28px_rgba(15,23,42,0.14)]">
          <div className="px-1.5 pb-0.5">
            <div className="flex items-center justify-between px-1 py-1 text-[12px]">
              <span className="font-medium text-text-base">模型</span>
              <span className="max-w-[88px] truncate text-text-secondary">{modelLabel(selectedModel) || "未选择"}</span>
            </div>

            <div className="max-h-[96px] overflow-y-auto py-0.5">
              {models.length === 0 ? (
                <div className="rounded-md px-2 py-1.5 text-[12px] text-text-secondary">{noModelsLabel}</div>
              ) : (
                models.map((id) => {
                  const selected = id === selectedModel;
                  return (
                    <button
                      type="button"
                      key={id}
                      disabled={switching}
                      onClick={() => onChooseModel(id)}
                      className={`flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left text-[12px] transition-colors ${
                        selected ? "bg-gray-100 text-text-base" : "text-text-secondary hover:bg-gray-50 hover:text-text-base"
                      }`}
                    >
                      <span className="truncate font-medium">{modelLabel(id)}</span>
                      {selected && <FontAwesomeIcon icon={["fas", "check"]} className="ml-3 text-[11px] text-text-base" />}
                    </button>
                  );
                })
              )}
            </div>
          </div>

          <div className="mx-1.5 my-1 h-px bg-border-theme" />

          <div className="px-1.5 pt-0.5 pb-0.5">
            <div className="mb-1 flex items-center justify-between px-1 text-[12px]">
              <span className="font-medium text-text-base">推理强度</span>
              <span className="text-text-secondary">{selectedThinkingOption.label}</span>
            </div>
            <div className="grid grid-cols-3 gap-0.5 rounded-full bg-gray-100 p-0.5">
              {thinkingOptions.map((option) => {
                const selected = option.id === selectedThinking;
                return (
                  <button
                    type="button"
                    key={option.id}
                    onClick={() => onChooseThinking(option.id)}
                    className={`flex h-7 items-center justify-center rounded-full text-[12px] font-medium transition-colors ${
                      selected
                        ? "bg-white text-text-base shadow-[0_1px_4px_rgba(15,23,42,0.12)]"
                        : "text-text-secondary hover:text-text-base"
                    }`}
                  >
                    {option.label}
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
