// MarketSkillCard — a single tile in the Skills Market grid.
//
// Renders a skill discovered via the SkillsMP REST API: name, author,
// description, ⭐ stars, last-updated date, plus an install / installed
// affordance. Click `+` flows up via `onInstall` so the parent can run the
// scan → AI review → install pipeline.

import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { MarketSkill } from "../../types";

export interface MarketSkillCardProps {
  skill: MarketSkill;
  installed: boolean;
  onInstall: (skill: MarketSkill) => void;
}

function formatStars(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return n.toString();
}

function formatUpdated(epochSeconds: number): string {
  const ms = epochSeconds * 1000;
  const date = new Date(ms);
  // Locale-aware ymd. Falls back to ISO if the locale renders empty.
  try {
    return new Intl.DateTimeFormat(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    }).format(date);
  } catch {
    return date.toISOString().slice(0, 10);
  }
}

export function MarketSkillCard({ skill, installed, onInstall }: MarketSkillCardProps) {
  const { t } = useTranslation();
  return (
    <div
      className="flex flex-col h-full rounded-xl border border-border-theme bg-white p-4 hover:shadow-sm hover:border-gray-300 transition-all"
      title={skill.skillUrl}
    >
      {/* Header: name + author */}
      <div className="flex items-start justify-between gap-2 mb-2">
        <div className="min-w-0 flex-1">
          <div className="text-[14px] font-medium text-text-base truncate">
            {skill.name}
          </div>
          <div className="text-[11px] text-text-secondary truncate mt-0.5">
            <FontAwesomeIcon icon={["fab", "github"]} className="mr-1 text-[10px]" />
            {skill.author}
          </div>
        </div>
        {installed ? (
          <span
            className="text-[11px] bg-green-50 border border-green-200 text-green-700 rounded-full px-2 py-0.5 flex-shrink-0 whitespace-nowrap"
            title={t("skillsView.market_already_installed")}
          >
            <FontAwesomeIcon icon={["fas", "check"]} className="mr-1" />
            {t("skillsView.market_already_installed")}
          </span>
        ) : (
          <button
            onClick={() => onInstall(skill)}
            title={t("skillsView.market_install_button")}
            className="w-7 h-7 rounded-full border border-border-theme flex items-center justify-center text-text-secondary bg-gray-50 hover:bg-blue-50 hover:text-blue-600 hover:border-blue-200 transition-all flex-shrink-0"
            aria-label={t("skillsView.market_install_button")}
          >
            <FontAwesomeIcon icon={["fas", "plus"]} className="text-xs" />
          </button>
        )}
      </div>

      {/* Description */}
      <p className="text-[12px] text-text-secondary line-clamp-3 leading-relaxed mb-3 flex-1">
        {skill.description}
      </p>

      {/* Footer: stars + updated */}
      <div className="flex items-center justify-between text-[11px] text-text-secondary mt-auto pt-2 border-t border-gray-100">
        <span className="flex items-center gap-1" title={`${skill.stars} stars`}>
          <FontAwesomeIcon icon={["fas", "star"]} className="text-yellow-500" />
          {formatStars(skill.stars)}
        </span>
        <span title={`Updated ${formatUpdated(skill.updatedAt)}`}>
          {formatUpdated(skill.updatedAt)}
        </span>
      </div>
    </div>
  );
}
