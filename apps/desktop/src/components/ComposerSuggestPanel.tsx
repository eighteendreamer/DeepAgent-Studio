import type { ReactNode } from "react";

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
  onSelect,
  onHover,
  className = "",
}: Props<T>) {
  if (!open) return null;

  return (
    <div
      className={`absolute left-3 bottom-full mb-2 max-h-60 w-[min(680px,calc(100%-1.5rem))] overflow-y-auto rounded-xl border border-border-theme bg-white py-1.5 shadow-[0_10px_30px_rgb(0,0,0,0.10)] z-50 ${className}`}
    >
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
              return (
                <button
                  key={getKey(item)}
                  type="button"
                  onMouseDown={(event) => {
                    event.preventDefault();
                    onSelect(item);
                  }}
                  onMouseEnter={() => onHover(flatIndex)}
                  className="w-full text-left"
                >
                  {renderItem(item, selected)}
                </button>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}
