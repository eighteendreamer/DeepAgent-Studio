import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import { Panel } from "./ui/Panel";
import { ListItem } from "./ui/ListItem";

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
    <Panel className="w-[332px] overflow-hidden rounded-2xl p-2">
      {cards.map((card) => {
        const label = t(`chatView.tools.${card.type}`, { defaultValue: card.title });
        const shortcut = shortcutMap[card.type];
        return (
          <ListItem
            key={`${card.type}:${card.title}`}
            onClick={() => onSelect(card)}
            className="gap-3 rounded-xl px-3 py-2.5 text-left cursor-pointer"
          >
            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-black/5 text-text-secondary">
              <FontAwesomeIcon icon={card.icon} className="text-[13px]" />
            </div>
            <span className="min-w-0 flex-1 truncate text-[14px] font-medium text-text-base">
              {label}
            </span>
            {shortcut ? (
              <span className="flex-shrink-0 text-[12px] text-text-secondary">{shortcut}</span>
            ) : null}
          </ListItem>
        );
      })}
    </Panel>
  );
}
