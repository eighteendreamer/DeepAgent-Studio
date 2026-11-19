/** Frontend demo data only. No ledger, Tauri command or provider API is consulted. */
export type UsageModel = "chat" | "reasoner";
export type UsageModelFilter = UsageModel | "all";
export type UsagePeriod = "7" | "30" | "90" | "year";
export interface UsageCounts { input: number; output: number; cacheRead: number }
export interface UsageDay { date: string; models: Record<UsageModel, UsageCounts> }
export interface UsageRange { start: string; end: string }
export interface UsagePoint extends UsageCounts { date: string; total: number }

const DAY_MS = 86_400_000;
export const USAGE_MODELS: UsageModel[] = ["chat", "reasoner"];
const emptyCounts = (): UsageCounts => ({ input: 0, output: 0, cacheRead: 0 });
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
    output: sum.output + value.output, cacheRead: sum.cacheRead + value.cacheRead }), emptyCounts());
  // Cache reads are a subset of input, never an additional source of total tokens.
  return { ...result, total: result.input + result.output };
}

export function dailyUsage(days: readonly UsageDay[], range: UsageRange, model: UsageModelFilter): UsagePoint[] {
  const byDate = new Map(days.map(day => [day.date, day]));
  const result: UsagePoint[] = [];
  for (let date = range.start; date <= range.end; date = shiftUsageDate(date, 1)) {
    const day = byDate.get(date);
    const counts = day ? (model === "all" ? USAGE_MODELS.map(key => day.models[key]) : [day.models[model]]) : [];
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

export function createDemoUsageDays(today: string): UsageDay[] {
  const start = shiftUsageDate(`${Number(today.slice(0, 4)) - 1}-01-01`, -1);
  let seed = 37;
  const random = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 4294967296; };
  const rows: UsageDay[] = [];
  for (let date = start; date <= today; date = shiftUsageDate(date, 1)) {
    const weekday = new Date(timestamp(date)).getUTCDay();
    const weekend = weekday === 0 || weekday === 6;
    const active = random() > (weekend ? .39 : .11);
    const amount = active ? Math.round((90_000 + random() * 950_000) * (weekend ? .58 : 1)) : 0;
    const share = .58 + random() * .22;
    const makeCounts = (ratio: number, reasoning: boolean): UsageCounts => {
      const total = Math.round(amount * ratio);
      const output = Math.round(total * (reasoning ? .24 + random() * .15 : .13 + random() * .1));
      const input = total - output;
      return { input, output, cacheRead: Math.round(input * (.38 + random() * .4)) };
    };
    rows.push({ date, models: { chat: makeCounts(share, false), reasoner: makeCounts(1 - share, true) } });
  }
  return rows;
}
