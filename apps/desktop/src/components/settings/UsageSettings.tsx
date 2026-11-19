import { useContext, useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { Box, ChevronDown, Info } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useThemeContext } from "../../theme/ThemeProvider";
import { mixHex } from "../../theme/colorUtils";
import { Button } from "../shadcn/button";
import { Progress } from "../shadcn/progress";
import { ActivityHeatmap } from "../shadcn/activity-heatmap";
import {
  DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuGroup,
  DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem,
} from "../shadcn/base-dropdown-menu";
import { SurfaceVisibility } from "../ui/SurfaceVisibility";
import { formatTokens } from "../chat-timeline/format";
import { UsageTrendChart } from "./UsageTrendChart";
import {
  createDemoUsageDays, dailyUsage, localUsageDate, previousUsageRange, sumUsage,
  usageRange, usageTrend, USAGE_MODELS, type UsageModelFilter, type UsagePeriod,
} from "./usageStatistics";
import "./UsageSettings.css";

function UsageDropdown<T extends string>({ value, onChange, options, label, icon }: {
  value: T; onChange: (value: T) => void;
  options: readonly { value: T; label: string }[];
  label: string; icon?: ReactNode;
}) {
  const visible = useContext(SurfaceVisibility);
  const [open, setOpen] = useState(false);
  useEffect(() => { if (!visible) setOpen(false); }, [visible]);
  return <DropdownMenu open={open && visible} onOpenChange={setOpen} modal={false}>
    <DropdownMenuTrigger render={<Button variant="ghost" size="sm" className="usage-dropdown" />}
      aria-label={label}>
      {icon}<span>{options.find(option => option.value === value)?.label}</span>
      <ChevronDown className="size-3.5" />
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end">
      <DropdownMenuGroup>
        <DropdownMenuLabel>{label}</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={value} onValueChange={(next: unknown) => {
          const option = options.find(item => item.value === next);
          if (option) onChange(option.value);
        }}>
          {options.map(option => <DropdownMenuRadioItem key={option.value} value={option.value}>
            {option.label}
          </DropdownMenuRadioItem>)}
        </DropdownMenuRadioGroup>
      </DropdownMenuGroup>
    </DropdownMenuContent>
  </DropdownMenu>;
}

function UsageMetric({ label, value, detail, primary = false }: {
  label: string; value: string; detail: ReactNode; primary?: boolean;
}) {
  return <div className={`usage-metric${primary ? " primary" : ""}`}>
    <span className="usage-metric-label">{label}</span>
    <strong className="usage-metric-value">{value}</strong>
    <span className="usage-metric-detail">{detail}</span>
  </div>;
}

export function UsageSettings() {
  const { t, i18n } = useTranslation();
  const { activePalette } = useThemeContext();
  // One set of hex colors serves both the CSS legends and the canvas renderer.
  const chartColors = useMemo(() => ({
    "--usage-chart-input": mixHex(activePalette.foreground, activePalette.background, .16),
    "--usage-chart-output": mixHex(activePalette.foreground, activePalette.background, .72),
  } as CSSProperties), [activePalette]);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const [today] = useState(() => localUsageDate(new Date()));
  const currentYear = Number(today.slice(0, 4));
  const [period, setPeriod] = useState<UsagePeriod>("30");
  const [model, setModel] = useState<UsageModelFilter>("all");
  const [year, setYear] = useState(String(currentYear));
  const [selectedDate, setSelectedDate] = useState<string | null>(today);
  const days = useMemo(() => createDemoUsageDays(today), [today]);
  const range = useMemo(() => usageRange(today, period), [today, period]);
  const points = useMemo(() => dailyUsage(days, range, model), [days, range, model]);
  const totals = useMemo(() => sumUsage(points), [points]);
  const previous = useMemo(() => sumUsage(dailyUsage(days, previousUsageRange(range), model)), [days, range, model]);
  const trend = useMemo(() => usageTrend(points, period === "year"), [points, period]);
  const annualPoints = useMemo(() => dailyUsage(days, {
    start: `${year}-01-01`, end: Number(year) === currentYear ? today : `${year}-12-31`,
  }, model), [days, year, currentYear, today, model]);
  const annualDays = useMemo(() => annualPoints.map(point => ({ date: point.date, count: point.total })), [annualPoints]);
  const annualTotal = useMemo(() => sumUsage(annualPoints).total, [annualPoints]);
  const modelTotals = useMemo(() => USAGE_MODELS.filter(key => model === "all" || key === model)
    .map(key => ({ key, ...sumUsage(dailyUsage(days, range, key)) })), [days, range, model]);
  const chartLabels = useMemo(() => ({ input: t("settings.usage.uncachedInput"),
    cache: t("settings.usage.cachedInput"), output: t("settings.usage.output"),
    summary: t("settings.usage.chartSummary"),
    error: t("settings.usage.chartError") }), [t]);
  const number = useMemo(() => new Intl.NumberFormat(locale), [locale]);
  const percentage = useMemo(() => new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1,
    minimumFractionDigits: 1 }), [locale]);
  const date = useMemo(() => new Intl.DateTimeFormat(locale, {
    year: "numeric", month: "short", day: "numeric", timeZone: "UTC",
  }), [locale]);
  const formatDate = (value: string) => date.format(Date.parse(`${value}T00:00:00Z`));
  const cacheRate = totals.input ? totals.cacheRead / totals.input : 0;
  const change = previous.total ? (totals.total - previous.total) / previous.total : null;
  const periods: { value: UsagePeriod; label: string }[] = [
    { value: "7", label: t("settings.usage.lastDays", { count: 7 }) },
    { value: "30", label: t("settings.usage.lastDays", { count: 30 }) },
    { value: "90", label: t("settings.usage.lastDays", { count: 90 }) },
    { value: "year", label: t("settings.usage.thisYear") },
  ];
  return <div className="usage-page" style={chartColors}>
    <header className="usage-heading">
      <div className="usage-title-line">
        <h1>{t("settings.usage.title")}</h1><span className="usage-demo-badge">{t("settings.usage.demo")}</span>
      </div>
      <p>{t("settings.usage.description")}</p>
    </header>

    <div>
      <div className="usage-toolbar">
        <div className="usage-periods" role="group" aria-label={t("settings.usage.period")}>
          {periods.map(option => <Button key={option.value} variant="ghost" size="sm"
            aria-pressed={period === option.value} onClick={() => setPeriod(option.value)}>
            {option.label}
          </Button>)}
        </div>
        <UsageDropdown value={model} onChange={setModel} label={t("settings.usage.modelFilter")}
          icon={<Box className="size-3.5" />} options={[
            { value: "all", label: t("settings.usage.allModels") },
            ...USAGE_MODELS.map(key => ({ value: key, label: `deepseek-${key}` })),
          ]} />
      </div>
      <div className="usage-date-line"><span>{formatDate(range.start)} — {formatDate(range.end)}</span>
        <span>{Intl.DateTimeFormat().resolvedOptions().timeZone}</span>
      </div>
    </div>

    <div className="usage-metrics" aria-live="polite">
      <UsageMetric primary label={t("settings.usage.total")} value={formatTokens(totals.total)} detail={<>
        {change === null ? "—" : <strong>{change >= 0 ? "↑" : "↓"} {percentage.format(Math.abs(change))}</strong>}
        {" "}{t("settings.usage.compared")}
      </>} />
      <UsageMetric label={t("settings.usage.input")} value={formatTokens(totals.input)}
        detail={t("settings.usage.includesCache", { value: formatTokens(totals.cacheRead) })} />
      <UsageMetric label={t("settings.usage.output")} value={formatTokens(totals.output)}
        detail={t("settings.usage.outputDescription")} />
      <UsageMetric label={t("settings.usage.cacheRate")} value={percentage.format(cacheRate)}
        detail={`${formatTokens(totals.cacheRead)} / ${formatTokens(totals.input)} ${t("settings.usage.inputShort")}`} />
    </div>

    <section className="usage-panel usage-activity" aria-labelledby="usage-activity-title">
      <div className="usage-panel-heading">
        <div><h2 id="usage-activity-title">{t("settings.usage.activity")}</h2>
          <p>{t("settings.usage.annualSummary", { year, days: annualPoints.filter(point => point.total > 0).length,
            total: formatTokens(annualTotal) })}</p>
        </div>
        <UsageDropdown value={year} label={t("settings.usage.heatmapYear")}
          onChange={value => { setYear(value); setSelectedDate(null); }}
          options={[currentYear, currentYear - 1].map(value => ({ value: String(value), label: String(value) }))} />
      </div>
      <ActivityHeatmap days={annualDays} year={Number(year)} today={today} locale={locale}
        label={t("settings.usage.activityLabel", { year })} selectedDate={selectedDate}
        onSelectDate={setSelectedDate} formatValue={value => number.format(value)} labels={{
          less: t("settings.usage.less"), more: t("settings.usage.more"),
          chooseDay: t("settings.usage.chooseDay"), future: t("settings.usage.future"),
          level: (level, count) => t("settings.usage.level", { level, count }),
        }} />
    </section>

    <div className="usage-bottom-grid">
      <section className="usage-panel" aria-labelledby="usage-trend-title">
        <div className="usage-panel-heading"><h2 id="usage-trend-title">{t("settings.usage.trend")}</h2>
          <span className="usage-muted">{t(period === "year" ? "settings.usage.monthly" : "settings.usage.daily")}</span>
        </div>
        <div className="usage-chart-legend">
          {(["input", "cache", "output"] as const).map(key => <span key={key}>
            <i className={`usage-dot ${key}`} />{chartLabels[key]}
          </span>)}
        </div>
        <UsageTrendChart points={trend} locale={locale} labels={chartLabels} />
      </section>
      <section className="usage-panel usage-models" aria-labelledby="usage-models-title">
        <div className="usage-panel-heading"><h2 id="usage-models-title">{t("settings.usage.modelDistribution")}</h2>
          <span className="usage-muted">{t("settings.usage.currentRange")}</span>
        </div>
        <div className="usage-model-total"><strong>{formatTokens(totals.total)}</strong><span>Token</span></div>
        <div className="usage-model-list">
          {modelTotals.map(item => {
            const share = totals.total ? item.total / totals.total * 100 : 0;
            return <div key={item.key} className={`usage-model-row ${item.key}`}>
              <div className="usage-model-label"><span><i className={`usage-dot ${item.key === "chat" ? "input" : "cache"}`} />deepseek-{item.key}</span>
                <span>{percentage.format(share / 100)}</span></div>
              <Progress value={share} aria-label={`deepseek-${item.key}`} className="usage-model-progress" />
              <div className="usage-model-value">{number.format(item.total)} Token</div>
            </div>;
          })}
        </div>
        <p className="usage-model-note">{t("settings.usage.distributionNote")}</p>
      </section>
    </div>
    <footer className="usage-footnote"><Info className="size-3.5 shrink-0" />
      <span>{t("settings.usage.countingNote")}</span>
    </footer>
  </div>;
}
