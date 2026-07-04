import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";

export type SidebarHeaderTab = {
  id: string;
  type: string;
  title: string;
  icon: IconProp;
};

interface Props {
  tabs: SidebarHeaderTab[];
  activeTabId: string;
  onSelectTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onShowLauncher: () => void;
  extraActions?: ReactNode;
  className?: string;
}

export function SidebarPluginHeader({
  tabs,
  activeTabId,
  onSelectTab,
  onCloseTab,
  onShowLauncher,
  extraActions = null,
  className = "",
}: Props) {
  const { t } = useTranslation();

  return (
    <div className={`border-b border-border-theme bg-white ${className}`.trim()}>
      <div className="flex h-[56px] items-center justify-between gap-3 pl-4 pr-2">
        <div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto no-scrollbar">
          {tabs.map((tab) => {
            const active = activeTabId === tab.id;
            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => onSelectTab(tab.id)}
                className={`group relative flex h-10 min-w-0 max-w-[240px] flex-shrink-0 items-center gap-2 rounded-xl px-4 text-[13px] transition-colors ${
                  active
                    ? "bg-[#f3f4f6] text-text-base shadow-[inset_0_0_0_1px_rgba(15,23,42,0.04)]"
                    : "text-text-secondary hover:bg-[#f7f8fa] hover:text-text-base"
                }`}
              >
                <FontAwesomeIcon icon={tab.icon} className="flex-shrink-0 text-[12px]" />
                <span className="min-w-0 flex-1 truncate text-left font-medium">{tab.title}</span>
                <span
                  onClick={(event) => {
                    event.stopPropagation();
                    onCloseTab(tab.id);
                  }}
                  className={`flex h-4 w-4 flex-shrink-0 items-center justify-center rounded text-[10px] transition-colors ${
                    active
                      ? "text-text-secondary hover:bg-white hover:text-text-base"
                      : "text-transparent group-hover:text-text-secondary hover:bg-white hover:text-text-base"
                  }`}
                >
                  <FontAwesomeIcon icon={["fas", "xmark"]} />
                </span>
              </button>
            );
          })}

          <button
            type="button"
            onClick={onShowLauncher}
            title={t("chatView.recommended", { defaultValue: "Open plugin" })}
            className="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-xl text-text-secondary transition-colors hover:bg-[#f3f4f6] hover:text-text-base"
          >
            <FontAwesomeIcon icon={["fas", "plus"]} className="text-[12px]" />
          </button>
        </div>

        <div className="flex flex-shrink-0 items-center gap-0.5">{extraActions}</div>
      </div>
    </div>
  );
}
