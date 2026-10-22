import { useLayoutEffect, useMemo, useRef, type ReactNode } from "react";
import { Panel } from "./ui/Panel";
import { HoverInfo } from "./ui/HoverInfo";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";

export type ComposerSuggestSection<T> = {
  key: string;
  label: string;
  items: T[];
};



interface Props<T> {
  id: string;
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
  id,
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
  const panelRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
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

  useLayoutEffect(() => {
    const panel = panelRef.current;
    const anchor = panel?.parentElement;
    if (!open || !panel || !anchor) return;
    const resize = () => {
      const top = anchor.closest(".app-main-surface")?.getBoundingClientRect().top ?? 0;
      // Leave 8px above/below the shell, plus its two 6px padding gutters.
      const available = Math.max(0, anchor.getBoundingClientRect().top - Math.max(0, top) - 28);
      panel.style.setProperty("--suggest-list-height", `${Math.min(320, available)}px`);
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(anchor);
    window.addEventListener("resize", resize);
    return () => { observer.disconnect(); window.removeEventListener("resize", resize); };
  }, [open]);

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    const selected = scroll?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!open || !scroll || !selected) return;
    const bounds = scroll.getBoundingClientRect(), row = selected.getBoundingClientRect();
    if (row.top < bounds.top) scroll.scrollTop -= bounds.top - row.top;
    else if (row.bottom > bounds.bottom) scroll.scrollTop += row.bottom - bounds.bottom;
  }, [open, activeId]);

  if (!open) return null;
  return (
    <>
      <Panel
        ref={panelRef}
        data-composer-suggest
        className={`absolute inset-x-0 bottom-full z-50 mb-2 w-auto overflow-hidden p-1.5 ${className}`}
      >
      <div ref={scrollRef} className="composer-suggest-scroll custom-scrollbar max-h-[var(--suggest-list-height,240px)] overflow-y-auto overscroll-contain">
      <SlidingMenuList id={id} role="listbox" aria-label={sections.map(section => section.label).join("、")} activeId={activeId} pillClassName="left-0 right-0">
        {sections.map((section, sectionIndex) => {
          let baseIndex = 0;
          for (let i = 0; i < sectionIndex; i += 1) {
            baseIndex += sections[i].items.length;
          }
          return (
            <div key={section.key} className={sectionIndex > 0 ? "mt-1" : ""}>
              <div className="px-2 py-1.5 text-[12px] font-medium text-text-secondary">
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
                    role="option"
                    id={`${id}-option-${flatIndex}`}
                    aria-selected={selected}
                    tabIndex={-1}
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
                  <HoverInfo key={itemKey} content={renderTooltip(item)}>{button}</HoverInfo>
                ) : (
                  button
                );
              })}
            </div>
          );
        })}
      </SlidingMenuList>
      </div>
      </Panel>
    </>
  );
}
