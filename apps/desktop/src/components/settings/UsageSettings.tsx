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
import { getUsageStatistics } from "../../api";
import { formatUsd, formatTokens } from "../chat-timeline/format";
import { UsageTrendChart } from "./UsageTrendChart";
import {
  dailyUsage, localUsageDate, shiftUsageDate, sumUsage,
  usageRange, usageTrend, type UsageDay, type UsagePeriod,
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
  const visible = useContext(SurfaceVisibility);
  const today = localUsageDate(new Date());
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const currentYear = Number(today.slice(0, 4));
  const [period, setPeriod] = useState<UsagePeriod>("30");
  const [model, setModel] = useState("");
  const [year, setYear] = useState(String(currentYear));
  const [selectedDate, setSelectedDate] = useState(today);
  const [days, setDays] = useState<UsageDay[]>([]);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!visible) return;
    let alive = true;
    const start = new Date(currentYear - 1, 0, 1).getTime();
    const end = new Date(`${shiftUsageDate(today, 1)}T00:00:00`).getTime();
    void getUsageStatistics(start, end, timezone).then(rows => {
      if (alive) { setDays(rows); setFailed(false); }
    }, () => { if (alive) setFailed(true); });
    return () => { alive = false; };
  }, [visible, currentYear, today, timezone]);
  const models = useMemo(() => [...new Set(days.flatMap(day => Object.keys(day.models)))].sort(), [days]);
  const selectedModel = models.includes(model) ? model : null;
  const range = useMemo(() => usageRange(today, period), [today, period]);
  const points = useMemo(() => dailyUsage(days, range, selectedModel), [days, range, selectedModel]);
  const totals = useMemo(() => sumUsage(points), [points]);
  const trend = useMemo(() => usageTrend(points, period === "year"), [points, period]);
  const annualPoints = useMemo(() => dailyUsage(days, {
    start: `${year}-01-01`, end: Number(year) === currentYear ? today : `${year}-12-31`,
  }, selectedModel), [days, year, currentYear, today, selectedModel]);
  const annualDays = useMemo(() => annualPoints.map(point => ({ date: point.date, count: point.total })), [annualPoints]);
  const annualTotal = useMemo(() => sumUsage(annualPoints).total, [annualPoints]);
  const selectedDay = annualPoints.find(point => point.date === selectedDate)!;
  const modelTotals = useMemo(() => models.filter(key => selectedModel === null || key === selectedModel)
    .map(key => ({ key, ...sumUsage(dailyUsage(days, range, key)) }))
    .filter(row => row.total > 0).sort((a, b) => b.total - a.total), [days, range, models, selectedModel]);
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
  const percent = (read: number, input: number) => input ? percentage.format(read / input) : "—";
  const cost = (counts: { costUsd: number | null; unpricedRequests: number }) =>
    counts.costUsd == null ? (counts.unpricedRequests ? t("settings.usage.unpriced") : "—")
      : `${counts.unpricedRequests ? "≥" : "≈"}${formatUsd(counts.costUsd)}`;
  const periods: { value: UsagePeriod; label: string }[] = [
    { value: "7", label: t("settings.usage.lastDays", { count: 7 }) },
    { value: "30", label: t("settings.usage.lastDays", { count: 30 }) },
    { value: "90", label: t("settings.usage.lastDays", { count: 90 }) },
    { value: "year", label: t("settings.usage.thisYear") },
  ];
  return <div className="usage-page" style={chartColors}>
    <header className="usage-heading">
      <div className="usage-title-line">
        <h1>{t("settings.usage.title")}</h1>
      </div>
      <p>{t("settings.usage.description")}</p>
    </header>
    {failed && <p role="alert">{t("settings.usage.loadError")}</p>}

    <div>
      <div className="usage-toolbar">
        <div className="usage-periods" role="group" aria-label={t("settings.usage.period")}>
          {periods.map(option => <Button key={option.value} variant="ghost" size="sm"
            aria-pressed={period === option.value} onClick={() => setPeriod(option.value)}>
            {option.label}
          </Button>)}
        </div>
        <UsageDropdown value={selectedModel ?? ""} onChange={setModel} label={t("settings.usage.modelFilter")}
          icon={<Box className="size-3.5" />} options={[
            { value: "", label: t("settings.usage.allModels") },
            ...models.map(key => ({ value: key, label: key })),
          ]} />
      </div>
      <div className="usage-date-line"><span>{formatDate(range.start)} — {formatDate(range.end)}</span>
        <span>{timezone}</span>
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
          <div><dt>{t("settings.usage.cacheRate")}</dt><dd>{percent(totals.cacheRead, totals.input)}</dd></div>
          <div><dt>{t("settings.usage.estimatedCost")}</dt><dd>{cost(totals)}</dd></div>
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
            <dd>{percent(selectedDay.cacheRead, selectedDay.input)}</dd></div>
          <div><dt>{t("settings.usage.cacheMiss")}</dt><dd>{formatTokens(selectedDay.cacheMiss)}</dd></div>
          <div><dt>{t("settings.usage.cacheWrite")}</dt><dd>{formatTokens(selectedDay.cacheWrite)}</dd></div>
          <div><dt>{t("settings.usage.estimatedCost")}</dt><dd>{cost(selectedDay)}</dd></div>
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
        <div className="usage-model-scroll"><table className="usage-model-table" aria-label={t("settings.usage.rangeModels")}>
          <thead><tr><th scope="col">{t("settings.usage.rangeModels")}</th>
            {["tokenUsage", "input", "output", "dayCache", "cacheMiss", "cacheRate", "estimatedCost"].map(key =>
              <th key={key} scope="col">{t(`settings.usage.${key}`)}</th>)}</tr></thead>
          <tbody>{modelTotals.map(item => <tr key={item.key}>
            <th scope="row">{item.key}</th>
            {[item.total, item.input, item.output, item.cacheRead, item.cacheMiss].map((value, index) =>
              <td key={index}>{formatTokens(value)}</td>)}
            <td>{percent(item.cacheRead, item.input)}</td><td>{cost(item)}</td>
          </tr>)}</tbody>
        </table></div>
      </section>
    </div>
    <footer className="usage-footnote"><Info className="size-3.5 shrink-0" />
      <span>{t("settings.usage.countingNote")}</span>
    </footer>
  </div>;
}
