import { useContext, useEffect, useRef, useState } from "react";
import { useReducedMotion } from "framer-motion";
import type { EChartsCoreOption, EChartsType } from "echarts/core";
import { useThemeContext } from "../../theme/ThemeProvider";
import { SurfaceVisibility } from "../ui/SurfaceVisibility";
import { formatTokens } from "../chat-timeline/format";
import type { UsagePoint } from "./usageStatistics";

let runtime: Promise<typeof import("./usageChartRuntime")> | undefined;
function loadTrendRuntime() {
  return runtime ??= import("./usageChartRuntime")
    .catch(error => { runtime = undefined; throw error; });
}

interface Props {
  points: readonly UsagePoint[];
  locale: string;
  labels: { total: string; summary: string; error: string };
}

export function UsageTrendChart({ points, locale, labels }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const instance = useRef<EChartsType | null>(null);
  const { activePalette, activeIsDark } = useThemeContext();
  const visible = useContext(SurfaceVisibility);
  const reduced = useReducedMotion();
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!visible) return;
    let cancelled = false;
    let observer: ResizeObserver | undefined;
    void loadTrendRuntime().then(core => {
      const element = host.current;
      if (cancelled || !element) return;
      setError(null);
      const styles = getComputedStyle(element);
      const color = (token: string) => styles.getPropertyValue(token).trim();
      const input = color("--usage-chart-input");
      const foreground = color("--theme-fg"), secondary = color("--theme-text-secondary");
      const border = color("--theme-border"), background = color("--theme-elevated");
      const number = new Intl.NumberFormat(locale);
      const dateLabel = (date: string) => date.length === 7
        ? new Intl.DateTimeFormat(locale, { month: "short", timeZone: "UTC" }).format(Date.parse(`${date}-01T00:00:00Z`))
        : new Intl.DateTimeFormat(locale, { month: "numeric", day: "numeric", timeZone: "UTC" }).format(Date.parse(`${date}T00:00:00Z`));
      const option: EChartsCoreOption = {
        animation: !reduced, animationDuration: 0, animationDurationUpdate: 300,
        color: [input], textStyle: { fontFamily: getComputedStyle(element).fontFamily },
        tooltip: { trigger: "axis", confine: true, backgroundColor: background, borderColor: border,
          textStyle: { color: foreground, fontSize: 12 },
          valueFormatter: (value: unknown) => `${number.format(Number(value))} Token` },
        grid: { left: 3, right: 8, top: 28, bottom: 4, containLabel: true },
        xAxis: { type: "category", data: points.map(point => dateLabel(point.date)), boundaryGap: false,
          axisLine: { show: false }, axisTick: { show: false },
          axisLabel: { color: secondary, fontSize: 11, margin: 12, hideOverlap: true } },
        yAxis: { type: "value", name: "Token", splitNumber: 3,
          nameTextStyle: { color: secondary, fontSize: 11, align: "left" },
          axisLabel: { color: secondary, fontSize: 11, formatter: formatTokens },
          splitLine: { lineStyle: { color: border, opacity: .45 } } },
        series: [{ id: "total", name: labels.total, type: "line", data: points.map(point => point.total),
          smooth: .3, symbol: "none", lineStyle: { width: 2 }, areaStyle: { opacity: .07 } }],
      };
      instance.current ??= core.init(element);
      instance.current.setOption(option);
      observer = new ResizeObserver(() => { if (element.clientWidth > 0) instance.current?.resize(); });
      observer.observe(element);
      instance.current.resize();
    }).catch(reason => {
      if (!cancelled) {
        console.error("Usage trend chart rendering failed:", reason);
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    });
    return () => { cancelled = true; observer?.disconnect(); };
  }, [points, locale, labels, visible, reduced, activePalette, activeIsDark]);
  useEffect(() => () => { instance.current?.dispose(); instance.current = null; }, []);
  return <>
    <div ref={host} className="usage-trend-chart" role="img" aria-label={labels.summary} />
    {error && <p role="alert" className="usage-chart-error">{labels.error}: {error}</p>}
  </>;
}
