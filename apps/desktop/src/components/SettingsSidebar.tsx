import {
  Anchor,
  Archive,
  ArrowLeft,
  Compass,
  type LucideIcon,
  GitBranch,
  GitFork,
  Info,
  Keyboard,
  Leaf,
  Link,
  Monitor,
  Network,
  Server,
  Settings,
  SlidersHorizontal,
  Smile,
  Sun,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useSlidingIndicator, SlidingPill } from "./ui/SlidingPill";

interface Category {
  id: string;
  label: string;
  icon: LucideIcon;
}

const CATEGORIES: Category[] = [
  { id: "general", label: "常规", icon: Settings },
  { id: "appearance", label: "外观", icon: Sun },
  { id: "config", label: "配置", icon: SlidersHorizontal },
  { id: "personalize", label: "个性化", icon: Smile },
  { id: "shortcuts", label: "键盘快捷键", icon: Keyboard },
  { id: "mcp", label: "MCP 服务器", icon: Server },
  { id: "hooks", label: "钩子", icon: Anchor },
  { id: "connections", label: "连接", icon: Link },
  { id: "git", label: "Git", icon: GitBranch },
  { id: "env", label: "环境", icon: Leaf },
  { id: "worktree", label: "工作树", icon: GitFork },
  { id: "browser", label: "浏览器", icon: Compass },
  { id: "computer", label: "电脑操控", icon: Monitor },
  { id: "project_map_debug", label: "项目地图调试", icon: Network },
  { id: "archive", label: "已归档对话", icon: Archive },
  { id: "about", label: "关于", icon: Info },
];

interface Props {
  onBack: () => void;
  activeCategoryId: string;
  onSelectCategory: (id: string) => void;
}

export function SettingsSidebar({ onBack, activeCategoryId, onSelectCategory }: Props) {
  const { t } = useTranslation();

  /* 滑动药丸指示器（静默着色）：悬停跟随，离开滑回激活项 */
  const { containerRef: listRef, containerProps, indicatorStyle } = useSlidingIndicator({
    hoverSelector: "[data-cat]",
    activeSelector: `[data-cat="${activeCategoryId}"]`,
  });

  return (
    <aside className="w-[240px] flex flex-col bg-sidebar-bg h-full no-select flex-shrink-0 pb-2">
      <div className="px-3 pt-4 pb-4">
        <button
          onClick={onBack}
          className="flex items-center text-text-secondary hover:text-text-base transition-colors text-[13px] font-medium px-2"
        >
          <ArrowLeft className="mr-2 h-4 w-4" />
          {t("settings.tabs.back")}
        </button>
      </div>

      <div
        ref={listRef}
        {...containerProps}
        className="relative flex-1 overflow-y-auto px-3"
      >
        <div className="space-y-0.5">
          {CATEGORIES.map((cat) => {
            const Icon = cat.icon;
            return (
              <button
                key={cat.id}
                data-cat={cat.id}
                onClick={() => onSelectCategory(cat.id)}
                className={`relative z-[1] w-full flex items-center px-2.5 py-1.5 rounded-md text-[13px] text-text-base ${
                  activeCategoryId === cat.id ? "font-medium" : ""
                }`}
              >
                <Icon className="mr-2 h-4 w-4 text-text-secondary" />
                <span>{t(`settings.tabs.${cat.id}`)}</span>
              </button>
            );
          })}
        </div>

        {/* 滑动药丸指示器 */}
        <SlidingPill style={indicatorStyle} className="bg-sidebar-highlight" />
      </div>
    </aside>
  );
}
