import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";

export interface PluginQuickMenuItem {
  icon: IconProp;
  title: string;
  type: string;
}

interface Props<T extends PluginQuickMenuItem = PluginQuickMenuItem> {
  cards: T[];
  onSelect: (card: T) => void;
  shortcutMap?: Record<string, string>;
}

export function PluginQuickMenu<T extends PluginQuickMenuItem>({
  cards,
  onSelect,
  shortcutMap = {},
}: Props<T>) {
  const { t } = useTranslation();

  return (
    <div className="w-[332px] overflow-hidden rounded-2xl border border-border-theme bg-white p-2 shadow-[0_18px_38px_rgba(15,23,42,0.14)]">
      {cards.map((card) => {
        const label = t(`chatView.tools.${card.type}`, { defaultValue: card.title });
        const shortcut = shortcutMap[card.type];
        return (
          <button
            key={`${card.type}:${card.title}`}
            type="button"
            onClick={() => onSelect(card)}
            className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-colors hover:bg-gray-50"
          >
            <div className="flex h-8 w-8 items-center justify-center rounded-lg border border-border-theme bg-white text-text-secondary">
              <FontAwesomeIcon icon={card.icon} className="text-[13px]" />
            </div>
            <span className="min-w-0 flex-1 truncate text-[14px] font-medium text-text-base">
              {label}
            </span>
            {shortcut ? (
              <span className="flex-shrink-0 text-[12px] text-text-secondary">{shortcut}</span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
