import { useState, useRef, useEffect, type ReactNode } from "react";
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
  onShowLauncher?: () => void;
  availablePlugins?: { id?: string; type: string; icon: IconProp; title: string; desc: string }[];
  onSelectPlugin?: (plugin: { id?: string; type: string; icon: IconProp; title: string; desc: string }) => void;
  extraActions?: ReactNode;
  reserveRightActionsSpace?: boolean;
  className?: string;
}

const FALLBACK_TOOL_NAMES: Record<string, string> = {
  files: "Files",
  chat: "Side Chat",
  browser: "Browser",
  terminal: "Terminal",
  project_map: "Project Map",
  recording: "Recording",
  file_preview: "File Preview",
};

function toolLabel(type: string) {
  return FALLBACK_TOOL_NAMES[type] ?? type;
}

function normalizeToolText(value: string) {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[\s-]+/g, "_")
    .toLowerCase();
}

function explicitLabel(value: string | undefined, type: string) {
  if (!value || value.startsWith("chatView.")) return "";
  const normalized = normalizeToolText(value);
  const fallback = normalizeToolText(toolLabel(type));
  if (normalized === type || normalized === fallback) return "";
  if (type === "chat" && normalized === "side_chat") return "";
  return value;
}

export function SidebarPluginHeader({
  tabs,
  activeTabId,
  onSelectTab,
  onCloseTab,
  onShowLauncher,
  availablePlugins,
  onSelectPlugin,
  extraActions = null,
  reserveRightActionsSpace = false,
  className = "",
}: Props) {
  const { t } = useTranslation();
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setIsMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  return (
    <div className={`min-w-0 max-w-full border-b border-border-theme bg-white ${className}`.trim()}>
      <div
        className={`grid h-8 min-w-0 max-w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-2 pl-4 ${
          reserveRightActionsSpace ? "pr-24" : "pr-2"
        }`}
      >
        <div className="min-w-0 flex-1 overflow-hidden">
          <div className="flex min-w-0 max-w-full items-center gap-1.5 overflow-x-auto overscroll-x-contain pr-1 no-scrollbar">
            {tabs.map((tab) => {
              const active = activeTabId === tab.id;
              return (
                <button
                  key={tab.id}
                  type="button"
                  onClick={() => onSelectTab(tab.id)}
                  className={`group relative flex h-6 min-w-[74px] max-w-[180px] flex-shrink-0 items-center gap-1.5 rounded-md px-2 text-[12px] transition-colors ${
                    active
                      ? "bg-[#f3f4f6] text-text-base shadow-[inset_0_0_0_1px_rgba(15,23,42,0.04)]"
                      : "text-text-secondary hover:bg-[#f7f8fa] hover:text-text-base"
                  }`}
                >
                  <FontAwesomeIcon icon={tab.icon} className="flex-shrink-0 text-[11px]" />
                  <span className="min-w-0 flex-1 truncate text-left font-medium">{tab.title}</span>
                  <span
                    onClick={(event) => {
                      event.stopPropagation();
                      onCloseTab(tab.id);
                    }}
                    className={`flex h-3.5 w-3.5 flex-shrink-0 items-center justify-center rounded text-[9px] transition-colors ${
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
          </div>
        </div>

        <div
          className={`flex flex-none items-center justify-end gap-2 ${
            extraActions ? "w-[78px]" : "w-7"
          }`}
        >
          <div className="relative flex-shrink-0" ref={menuRef}>
            <button
              type="button"
              onClick={() => {
                if (availablePlugins && availablePlugins.length > 0) {
                  setIsMenuOpen(!isMenuOpen);
                } else if (onShowLauncher) {
                  onShowLauncher();
                }
              }}
              title={t("chatView.recommended", { defaultValue: "Open plugin" })}
              className={`flex h-7 w-7 flex-shrink-0 items-center justify-center rounded-md transition-colors ${
                isMenuOpen
                  ? "bg-[#f3f4f6] text-text-base"
                  : "text-text-secondary hover:bg-[#f3f4f6] hover:text-text-base"
              }`}
            >
              <FontAwesomeIcon icon={["fas", "plus"]} className="text-[12px]" />
            </button>

            {isMenuOpen && availablePlugins && (
              <div className="popover-menu absolute right-0 top-full mt-1 flex w-56 origin-top-right flex-col rounded-xl border border-border-theme bg-white py-1.5 shadow-[0_8px_30px_rgb(0,0,0,0.12)] z-[100]">
                {availablePlugins.map((plugin) => (
                  <button
                    key={plugin.id ?? `${plugin.type}:${plugin.title}`}
                    type="button"
                    onClick={() => {
                      setIsMenuOpen(false);
                      onSelectPlugin?.(plugin);
                    }}
                    className="flex w-full items-center px-4 py-2 text-left transition-colors hover:bg-gray-50"
                  >
                    <div className="mr-3 flex h-6 w-6 items-center justify-center rounded-md bg-primary/10 text-primary">
                      <FontAwesomeIcon icon={plugin.icon} className="text-[12px]" />
                    </div>
                    <span className="text-[13px] font-medium text-text-base">
                      {explicitLabel(plugin.title, plugin.type) ||
                        t(`chatView.tools.${plugin.type}`, {
                          defaultValue: toolLabel(plugin.type),
                        })}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>

          {extraActions && (
            <>
              <div className="mx-0.5 h-5 w-px flex-shrink-0 bg-border-theme" />
              <div className="flex flex-shrink-0 items-center gap-1.5">{extraActions}</div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
