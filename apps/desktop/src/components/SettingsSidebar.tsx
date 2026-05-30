import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";

interface Category {
  id: string;
  label: string;
  icon: IconProp;
}

const CATEGORIES: Category[] = [
  { id: "general", label: "常规", icon: ["fas", "gear"] },
  { id: "appearance", label: "外观", icon: ["far", "sun"] },
  { id: "config", label: "配置", icon: ["fas", "sliders"] },
  { id: "personalize", label: "个性化", icon: ["far", "face-smile"] },
  { id: "shortcuts", label: "键盘快捷键", icon: ["far", "keyboard"] },
  { id: "mcp", label: "MCP 服务器", icon: ["fas", "server"] },
  { id: "hooks", label: "钩子", icon: ["fas", "anchor"] },
  { id: "connections", label: "连接", icon: ["fas", "link"] },
  { id: "git", label: "Git", icon: ["fab", "git-alt"] },
  { id: "env", label: "环境", icon: ["fas", "leaf"] },
  { id: "worktree", label: "工作树", icon: ["fas", "code-branch"] },
  { id: "browser", label: "浏览器", icon: ["far", "compass"] },
  { id: "computer", label: "电脑操控", icon: ["fas", "desktop"] },
  { id: "archive", label: "已归档对话", icon: ["fas", "box-archive"] },
];

interface Props {
  onBack: () => void;
  activeCategoryId: string;
  onSelectCategory: (id: string) => void;
}

export function SettingsSidebar({ onBack, activeCategoryId, onSelectCategory }: Props) {
  const { t } = useTranslation();

  return (
    <aside className="w-[240px] flex flex-col bg-sidebar-bg h-full no-select flex-shrink-0 pb-2">
      <div className="px-3 pt-4 pb-4">
        <button 
          onClick={onBack} 
          className="flex items-center text-text-secondary hover:text-text-base transition-colors text-[13px] font-medium px-2"
        >
          <FontAwesomeIcon icon={["fas", "arrow-left"]} className="mr-2" />
          {t("settings.tabs.back")}
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-3 space-y-0.5">
        {CATEGORIES.map((cat) => (
          <button
            key={cat.id}
            onClick={() => onSelectCategory(cat.id)}
            className={`w-full flex items-center px-2.5 py-1.5 rounded-md text-[13px] transition-colors ${
              activeCategoryId === cat.id
                ? "bg-black/5 text-text-base font-medium"
                : "text-text-base hover:bg-black/5"
            }`}
          >
            <FontAwesomeIcon icon={cat.icon} className="w-5 text-left text-text-secondary" />
            <span className="ml-1">{t(`settings.tabs.${cat.id}`)}</span>
          </button>
        ))}
      </div>
    </aside>
  );
}
