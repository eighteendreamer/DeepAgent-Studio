import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";

export interface ToolLauncherCard {
  icon: IconProp;
  title: string;
  desc: string;
  type: string;
}

interface Props<T extends ToolLauncherCard = ToolLauncherCard> {
  cards: T[];
  onSelect: (card: T) => void;
  variant?: "sidebar" | "bottom";
}

const FALLBACK_TOOL_NAMES: Record<string, string> = {
  files: "Files",
  chat: "Side Chat",
  browser: "Browser",
  computer_use: "Computer Use",
  terminal: "Terminal",
  project_map: "Project Map",
  recording: "Recording",
  file_preview: "File Preview",
  canvas: "Studio Canvas",
};

const FALLBACK_TOOL_DESCRIPTIONS: Record<string, string> = {
  files: "Browse project files",
  chat: "Start a side chat",
  browser: "Open website",
  computer_use: "Control desktop apps",
  terminal: "Launch interactive shell",
  project_map: "Inspect module relationships",
  recording: "Meeting recording and transcription",
  file_preview: "Preview office documents",
  canvas: "Open extensible workspace",
};

function toolLabel(type: string) {
  return FALLBACK_TOOL_NAMES[type] ?? type;
}

function toolDescription(type: string) {
  return FALLBACK_TOOL_DESCRIPTIONS[type] ?? "";
}

function normalizeToolText(value: string) {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[\s-]+/g, "_")
    .toLowerCase();
}

function explicitLabel(value: string | undefined, type: string, suffix = "") {
  if (!value || value.startsWith("chatView.")) return "";
  const normalized = normalizeToolText(value);
  const fallback = normalizeToolText(suffix ? toolDescription(type) : toolLabel(type));
  const key = suffix ? `${type}${suffix}` : type;
  if (normalized === normalizeToolText(key) || normalized === fallback) return "";
  if (!suffix && type === "chat" && normalized === "side_chat") return "";
  return value;
}

const TOOL_SHORTCUTS: Record<string, string> = {
  files: "Ctrl+P",
  chat: "Ctrl+Alt+S",
  browser: "Ctrl+T",
  computer_use: "",
  project_map: "",
  terminal: "",
  recording: "",
  file_preview: "",
  canvas: "",
};

export function ToolLauncherPanel<T extends ToolLauncherCard>({
  cards,
  onSelect,
  variant = "sidebar",
}: Props<T>) {
  const { t } = useTranslation();
  const isSidebar = variant === "sidebar";

  return (
    <div className={`w-full h-full overflow-y-auto ${isSidebar ? "bg-white" : ""}`}>
      <div className={isSidebar ? "px-5 py-5" : "px-6 py-5"}>
        <div
          className={
            isSidebar
              ? "flex flex-col gap-3"
              : "grid gap-3 grid-cols-[repeat(auto-fit,minmax(190px,1fr))] max-w-5xl mx-auto"
          }
        >
          {cards.map((card) => {
            const ownTitle = explicitLabel(card.title, card.type);
            const ownDesc = explicitLabel(card.desc, card.type, "Desc");
            const translatedTitle =
              ownTitle ||
              t(`chatView.tools.${card.type}`, {
                defaultValue: toolLabel(card.type),
              });
            const translatedDesc =
              ownDesc ||
              t(`chatView.tools.${card.type}Desc`, {
                defaultValue: toolDescription(card.type),
              });
            const shortcut = TOOL_SHORTCUTS[card.type] || "";

            if (isSidebar) {
              return (
                <button
                  key={`${card.type}:${card.title}`}
                  type="button"
                  onClick={() => onSelect(card)}
                  className="group grid min-h-[72px] w-full grid-cols-[auto_1fr_auto] items-center gap-3 rounded-xl border border-border-theme bg-white px-4 py-3 text-left transition-all duration-200 hover:border-primary/45 hover:bg-[#fbfdff] hover:shadow-[0_10px_26px_rgba(15,23,42,0.08)]"
                >
                  <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 text-primary transition-transform duration-200 group-hover:scale-105">
                    <FontAwesomeIcon icon={card.icon} className="text-[17px]" />
                  </div>
                  <div className="min-w-0">
                    <div className="truncate text-[14px] font-semibold text-text-base transition-colors group-hover:text-primary">
                      {translatedTitle}
                    </div>
                    <div className="mt-0.5 truncate text-[12px] leading-5 text-text-secondary">
                      {translatedDesc}
                    </div>
                  </div>
                  <div className="flex items-center justify-end gap-2">
                    {shortcut && (
                      <span className="hidden text-[11px] font-mono tracking-wide text-text-secondary/60 xl:inline">
                        {shortcut}
                      </span>
                    )}
                    <div className="flex h-7 w-7 items-center justify-center rounded-full text-text-secondary transition-all duration-200 group-hover:bg-primary/10 group-hover:text-primary">
                      <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[11px]" />
                    </div>
                  </div>
                </button>
              );
            }

            return (
              <button
                key={`${card.type}:${card.title}`}
                type="button"
                onClick={() => onSelect(card)}
                className="group grid min-h-[88px] w-full grid-cols-[auto_1fr_auto] items-center gap-3 rounded-xl border border-border-theme bg-white px-4 py-3 text-left shadow-[0_1px_2px_rgba(15,23,42,0.03)] transition-all duration-200 hover:-translate-y-0.5 hover:border-primary/45 hover:shadow-[0_12px_28px_rgba(15,23,42,0.08)]"
              >
                <div className="flex h-11 w-11 items-center justify-center rounded-lg bg-primary/10 text-primary transition-transform duration-200 group-hover:scale-105">
                  <FontAwesomeIcon icon={card.icon} className="text-[18px]" />
                </div>
                <div className="min-w-0">
                  <div className="truncate text-[14px] font-semibold text-text-base transition-colors group-hover:text-primary">
                    {translatedTitle}
                  </div>
                  <div className="mt-0.5 line-clamp-2 text-[12px] leading-5 text-text-secondary">
                    {translatedDesc}
                  </div>
                </div>
                <div className="flex justify-end">
                  <div className="flex h-8 w-8 items-center justify-center rounded-full bg-black/[0.03] text-text-secondary transition-all duration-200 group-hover:bg-primary/10 group-hover:text-primary">
                    <FontAwesomeIcon icon={["fas", "arrow-right"]} className="text-[12px]" />
                  </div>
                </div>
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
