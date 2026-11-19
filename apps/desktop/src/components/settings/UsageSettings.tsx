import { useContext, useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { Box, ChevronDown, Info } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useThemeContext } from "../../theme/ThemeProvider";
import { mixHex } from "../../theme/colorUtils";
import { Button } from "../shadcn/button";
import { ActivityHeatmap } from "../shadcn/activity-heatmap";
import {
  DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuGroup,
  DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem,
} from "../shadcn/base-dropdown-menu";
import { SurfaceVisibility } from "../ui/SurfaceVisibility";
import { formatTokens } from "../chat-timeline/format";
import { UsageTrendChart } from "./UsageTrendChart";
import {
  createDemoUsageDays, dailyUsage, localUsageDate, sumUsage,
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

export function UsageSettings() {
  const { t, i18n } = useTranslation();
  const { activePalette } = useThemeContext();
  // Resolve the themed line color to hex for the canvas renderer.
  const chartColors = useMemo(() => ({
    "--usage-chart-input": mixHex(activePalette.foreground, activePalette.background, .16),
  } as CSSProperties), [activePalette]);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const [today] = useState(() => localUsageDate(new Date()));
  const currentYear = Number(today.slice(0, 4));
  const [period, setPeriod] = useState<UsagePeriod>("30");
  const [model, setModel] = useState<UsageModelFilter>("all");
  const [year, setYear] = useState(String(currentYear));
  const [selectedDate, setSelectedDate] = useState(today);
  const days = useMemo(() => createDemoUsageDays(today), [today]);
  const range = useMemo(() => usageRange(today, period), [today, period]);
  const points = useMemo(() => dailyUsage(days, range, model), [days, range, model]);
  const totals = useMemo(() => sumUsage(points), [points]);
  const trend = useMemo(() => usageTrend(points, period === "year"), [points, period]);
  const annualPoints = useMemo(() => dailyUsage(days, {
    start: `${year}-01-01`, end: Number(year) === currentYear ? today : `${year}-12-31`,
  }, model), [days, year, currentYear, today, model]);
  const annualDays = useMemo(() => annualPoints.map(point => ({ date: point.date, count: point.total })), [annualPoints]);
  const annualTotal = useMemo(() => sumUsage(annualPoints).total, [annualPoints]);
  const selectedDay = annualPoints.find(point => point.date === selectedDate)!;
  const modelTotals = useMemo(() => USAGE_MODELS.filter(key => model === "all" || key === model)
    .map(key => ({ key, ...sumUsage(dailyUsage(days, range, key)) }))
    .sort((a, b) => b.total - a.total), [days, range, model]);
  const chartLabels = useMemo(() => ({ total: t("settings.usage.total"),
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

    <section className="usage-panel usage-calendar" aria-labelledby="usage-activity-title">
      <div className="usage-calendar-intro">
        <div className="usage-calendar-total">
          <p>{t("settings.usage.rangeTotal")}</p>
          <div><strong>{formatTokens(totals.total)}</strong><span>Token</span></div>
        </div>
        <dl className="usage-calendar-stats">
          <div><dt>{t("settings.usage.activeDays")}</dt>
            <dd>{number.format(points.filter(point => point.total > 0).length)}<small>{t("settings.usage.daysUnit")}</small></dd></div>
          <div><dt>{t("settings.usage.dailyAverage")}</dt>
            <dd>{formatTokens(totals.total / Math.max(1, points.length))}</dd></div>
          <div><dt>{t("settings.usage.cacheRate")}</dt><dd>{percentage.format(cacheRate)}</dd></div>
        </dl>
      </div>
      <div className="usage-calendar-activity">
        <div className="usage-panel-heading">
          <div><h2 id="usage-activity-title">{t("settings.usage.activity", { year })}</h2>
            <p>{t("settings.usage.annualSummary", { year, days: annualPoints.filter(point => point.total > 0).length,
              total: formatTokens(annualTotal) })}</p>
          </div>
          <UsageDropdown value={year} label={t("settings.usage.heatmapYear")}
            onChange={value => { setYear(value); setSelectedDate(Number(value) === currentYear ? today : `${value}-12-31`); }}
            options={[currentYear, currentYear - 1].map(value => ({ value: String(value), label: String(value) }))} />
        </div>
        <ActivityHeatmap days={annualDays} year={Number(year)} today={today} locale={locale}
          label={t("settings.usage.activityLabel", { year })} selectedDate={selectedDate}
          onSelectDate={setSelectedDate} formatValue={value => number.format(value)} labels={{
            less: t("settings.usage.less"), more: t("settings.usage.more"),
            chooseDay: t("settings.usage.chooseDay"), future: t("settings.usage.future"),
            level: (level, count) => t("settings.usage.level", { level, count }),
          }} />
        <div className="usage-calendar-summary">
          <span>{t("settings.usage.activeDays")} <strong>{number.format(annualPoints.filter(point => point.total > 0).length)}</strong></span>
          <span>{t("settings.usage.annualTotal")} <strong>{formatTokens(annualTotal)}</strong> Token</span>
          <span>{t("settings.usage.heatmapHint")}</span>
        </div>
      </div>
    </section>

    <div className="usage-calendar-lower">
      <section className="usage-panel usage-day-panel" aria-label={t("settings.usage.selectedDay")} aria-live="polite" aria-atomic="true">
        <h2 className="usage-day-date">{formatDate(selectedDate)}</h2>
        <div className="usage-day-total"><strong>{formatTokens(selectedDay.total)}</strong><span>Token</span></div>
        <dl className="usage-day-details">
          <div><dt>{t("settings.usage.input")}</dt><dd>{formatTokens(selectedDay.input)}</dd></div>
          <div><dt>{t("settings.usage.output")}</dt><dd>{formatTokens(selectedDay.output)}</dd></div>
          <div><dt>{t("settings.usage.dayCache")}</dt><dd>{formatTokens(selectedDay.cacheRead)}</dd></div>
          <div><dt>{t("settings.usage.cacheRate")}</dt>
            <dd>{percentage.format(selectedDay.input ? selectedDay.cacheRead / selectedDay.input : 0)}</dd></div>
        </dl>
        <p className="usage-day-help">{t(selectedDay.total ? "settings.usage.dayHint" : "settings.usage.emptyDay")}</p>
      </section>
      <section className="usage-panel" aria-labelledby="usage-trend-title">
        <div className="usage-panel-heading">
          <div><h2 id="usage-trend-title">{t("settings.usage.trend")}</h2>
            <p>{t(period === "year" ? "settings.usage.monthly" : "settings.usage.daily")}</p></div>
          <span className="usage-muted">{formatTokens(totals.total)} Token</span>
        </div>
        <UsageTrendChart points={trend} locale={locale} labels={chartLabels} />
        <table className="usage-model-table" aria-label={t("settings.usage.rangeModels")}>
          <thead><tr><th scope="col">{t("settings.usage.rangeModels")}</th>
            <th scope="col">{t("settings.usage.tokenUsage")}</th></tr></thead>
          <tbody>{modelTotals.map(item => <tr key={item.key}>
            <th scope="row">deepseek-{item.key}</th><td>{formatTokens(item.total)}</td>
          </tr>)}</tbody>
        </table>
      </section>
    </div>
    <footer className="usage-footnote"><Info className="size-3.5 shrink-0" />
      <span>{t("settings.usage.countingNote")}</span>
    </footer>
  </div>;
}
