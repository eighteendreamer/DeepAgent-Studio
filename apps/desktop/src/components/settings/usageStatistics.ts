export type UsagePeriod = "7" | "30" | "90" | "year";
export interface UsageCounts {
  input: number; output: number; cacheRead: number; cacheMiss: number; cacheWrite: number;
  costUsd: number | null; unpricedRequests: number;
}
export interface UsageDay { date: string; models: Record<string, UsageCounts> }
export interface UsageRange { start: string; end: string }
export interface UsagePoint extends UsageCounts { date: string; total: number }

const DAY_MS = 86_400_000;
export const emptyCounts = (): UsageCounts => ({ input: 0, output: 0, cacheRead: 0,
  cacheMiss: 0, cacheWrite: 0, costUsd: null, unpricedRequests: 0 });
const timestamp = (date: string) => Date.parse(`${date}T00:00:00Z`);
export const shiftUsageDate = (date: string, days: number) =>
  new Date(timestamp(date) + days * DAY_MS).toISOString().slice(0, 10);

export function localUsageDate(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function usageRange(today: string, period: UsagePeriod): UsageRange {
  return { start: period === "year" ? `${today.slice(0, 4)}-01-01`
    : shiftUsageDate(today, 1 - Number(period)), end: today };
}

export function previousUsageRange(range: UsageRange): UsageRange {
  const days = Math.round((timestamp(range.end) - timestamp(range.start)) / DAY_MS) + 1;
  return { start: shiftUsageDate(range.start, -days), end: shiftUsageDate(range.start, -1) };
}

export function sumUsage(counts: readonly UsageCounts[]): UsageCounts & { total: number } {
  const result = counts.reduce((sum, value) => ({ input: sum.input + value.input,
    output: sum.output + value.output, cacheRead: sum.cacheRead + value.cacheRead,
    cacheMiss: sum.cacheMiss + value.cacheMiss, cacheWrite: sum.cacheWrite + value.cacheWrite,
    costUsd: value.costUsd == null ? sum.costUsd : (sum.costUsd ?? 0) + value.costUsd,
    unpricedRequests: sum.unpricedRequests + value.unpricedRequests }), emptyCounts());
  // Cache reads are a subset of input, never an additional source of total tokens.
  return { ...result, total: result.input + result.output };
}

export function dailyUsage(days: readonly UsageDay[], range: UsageRange, model: string | null): UsagePoint[] {
  const byDate = new Map(days.map(day => [day.date, day]));
  const result: UsagePoint[] = [];
  for (let date = range.start; date <= range.end; date = shiftUsageDate(date, 1)) {
    const day = byDate.get(date);
    const counts = day ? (model === null ? Object.values(day.models)
      : Object.prototype.hasOwnProperty.call(day.models, model) ? [day.models[model]] : []) : [];
    result.push({ date, ...sumUsage(counts) });
  }
  return result;
}

export function usageTrend(points: readonly UsagePoint[], monthly: boolean): UsagePoint[] {
  if (!monthly) return [...points];
  const months = new Map<string, UsageCounts[]>();
  for (const point of points) {
    const month = point.date.slice(0, 7);
    const group = months.get(month) ?? [];
    group.push(point);
    months.set(month, group);
  }
  return [...months].map(([date, rows]) => ({ date, ...sumUsage(rows) }));
}
