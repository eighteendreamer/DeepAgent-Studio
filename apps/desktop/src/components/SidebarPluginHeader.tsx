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
}

export function SidebarPluginHeader({
  tabs,
  activeTabId,
  onSelectTab,
  onCloseTab,
  onShowLauncher,
  extraActions = null,
}: Props) {
  const { t } = useTranslation();

  return (
    <div className="border-b border-border-theme bg-white">
      <div className="flex h-[44px] items-end justify-between gap-3 px-3">
        <div className="flex min-w-0 flex-1 items-end gap-1 overflow-x-auto no-scrollbar pt-1.5">
          {tabs.map((tab) => {
            const active = activeTabId === tab.id;
            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => onSelectTab(tab.id)}
                className={`group relative flex h-[34px] min-w-0 max-w-[220px] flex-shrink-0 items-center gap-2 rounded-t-[10px] border px-3 text-[13px] transition-colors ${
                  active
                    ? "border-border-theme border-b-white bg-[#f7f8fa] text-text-base"
                    : "border-transparent bg-transparent text-text-secondary hover:bg-[#f7f8fa] hover:text-text-base"
                }`}
              >
                <FontAwesomeIcon icon={tab.icon} className="flex-shrink-0 text-[12px]" />
                <span className="min-w-0 flex-1 truncate text-left font-medium">{tab.title}</span>
                <span
                  onClick={(event) => {
                    event.stopPropagation();
                    onCloseTab(tab.id);
                  }}
                  className="flex h-4 w-4 flex-shrink-0 items-center justify-center rounded text-[10px] text-text-secondary transition-colors hover:bg-white hover:text-text-base"
                >
                  <FontAwesomeIcon icon={["fas", "xmark"]} />
                </span>
              </button>
            );
          })}

          <button
            type="button"
            onClick={onShowLauncher}
            title={t("chatView.recommended", { defaultValue: "打开插件" })}
            className="mb-[3px] flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-[#f3f4f6] hover:text-text-base"
          >
            <FontAwesomeIcon icon={["fas", "plus"]} className="text-[12px]" />
          </button>
        </div>

        <div className="mb-[5px] flex flex-shrink-0 items-center gap-1.5">{extraActions}</div>
      </div>
    </div>
  );
}
