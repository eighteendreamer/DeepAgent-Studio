import { useMemo, type ReactNode } from "react";
import { Panel } from "./ui/Panel";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./ui/Tooltip";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";

export type ComposerSuggestSection<T> = {
  key: string;
  label: string;
  items: T[];
};



interface Props<T> {
  open: boolean;
  sections: ComposerSuggestSection<T>[];
  selectedIndex: number;
  getKey: (item: T) => string;
  renderItem: (item: T, selected: boolean) => ReactNode;
  renderTooltip?: (item: T) => ReactNode;
  onSelect: (item: T) => void;
  onHover: (index: number) => void;
  className?: string;
}

export function ComposerSuggestPanel<T>({
  open,
  sections,
  selectedIndex,
  getKey,
  renderItem,
  renderTooltip,
  onSelect,
  onHover,
  className = "",
}: Props<T>) {
  const activeId = useMemo(() => {
    let index = 0;
    for (const section of sections) {
      for (const item of section.items) {
        if (index === selectedIndex) return getKey(item);
        index += 1;
      }
    }
    return "";
  }, [sections, selectedIndex, getKey]);
  if (!open) return null;
  return (
    <TooltipProvider>
      <Panel
        className={`absolute left-3 bottom-full z-50 mb-2 max-h-60 w-[min(680px,calc(100%-1.5rem))] overflow-y-auto p-1.5 ${className}`}
      >
      <SlidingMenuList activeId={activeId}>
        {sections.map((section, sectionIndex) => {
          let baseIndex = 0;
          for (let i = 0; i < sectionIndex; i += 1) {
            baseIndex += sections[i].items.length;
          }
          return (
            <div key={section.key} className={sectionIndex > 0 ? "mt-1" : ""}>
              <div className="px-4 py-1 text-[12px] font-medium text-text-secondary">
                {section.label}
              </div>
              {section.items.map((item, index) => {
                const flatIndex = baseIndex + index;
                const selected = flatIndex === selectedIndex;
                const itemKey = getKey(item);
                const button = (
                  <button
                    key={itemKey}
                    type="button"
                    {...{ [MENU_ITEM_ATTR]: itemKey }}
                    onMouseDown={(event) => {
                      event.preventDefault();
                      onSelect(item);
                    }}
                    onMouseEnter={() => onHover(flatIndex)}
                    className="relative z-[1] w-full text-left"
                  >
                    {renderItem(item, selected)}
                  </button>
                );
                return renderTooltip ? (
                  <Tooltip key={itemKey}>
                    <TooltipTrigger asChild>{button}</TooltipTrigger>
                    <TooltipContent side="top" align="start">
                      {renderTooltip(item)}
                    </TooltipContent>
                  </Tooltip>
                ) : (
                  button
                );
              })}
            </div>
          );
        })}
      </SlidingMenuList>
      </Panel>
    </TooltipProvider>
  );
}
