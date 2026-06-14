// SkillRiskBadge — small chip displaying a single static-scan finding's
// category and severity (consumed by SkillInstallDialog's risk list).
//
// Severity drives color: safe = gray, warning = amber, danger = red.
// Category drives the leading icon. The full detail message (file/line/text)
// is exposed via the `title` attribute so the chip stays compact.

import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconPrefix, IconName } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { RiskCategory, RiskSeverity } from "../../types";

const CATEGORY_ICONS: Record<RiskCategory, [IconPrefix, IconName]> = {
  shell: ["fas", "terminal"],
  execution: ["fas", "play"],
  network: ["fas", "globe"],
  credential: ["fas", "key"],
  filesystem: ["fas", "folder-tree"],
  exfiltration: ["fas", "arrow-right-from-bracket"],
};

const SEVERITY_STYLES: Record<RiskSeverity, string> = {
  safe: "bg-gray-50 text-gray-600 border-gray-200",
  warning: "bg-amber-50 text-amber-700 border-amber-200",
  danger: "bg-red-50 text-red-700 border-red-200",
};

export interface SkillRiskBadgeProps {
  category: RiskCategory;
  severity: RiskSeverity;
  detail?: string;
  className?: string;
}

export function SkillRiskBadge({
  category,
  severity,
  detail,
  className = "",
}: SkillRiskBadgeProps) {
  const { t } = useTranslation();
  const [pack, name] = CATEGORY_ICONS[category];
  const style = SEVERITY_STYLES[severity];
  return (
    <span
      className={`inline-flex items-center gap-1 text-[11px] rounded-full px-2 py-0.5 border whitespace-nowrap ${style} ${className}`}
      title={detail ?? `${category} (${severity})`}
    >
      <FontAwesomeIcon icon={[pack, name]} className="text-[10px]" />
      <span>{t(`skillRisk.category.${category}`)}</span>
      <span className="opacity-60">·</span>
      <span>{t(`skillRisk.severity.${severity}`)}</span>
    </span>
  );
}
