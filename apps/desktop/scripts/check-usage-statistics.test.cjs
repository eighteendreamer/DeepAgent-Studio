const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const ts = require("typescript");

const source = fs.readFileSync(path.join(__dirname, "../src/components/settings/usageStatistics.ts"), "utf8");
const { outputText } = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
});
const implementation = { exports: {} };
new Function("exports", "module", outputText)(implementation.exports, implementation);
const { sumUsage, usageRange, previousUsageRange, dailyUsage, usageTrend,
  emptyCounts, shiftUsageDate, localUsageDate } = implementation.exports;

const priced = { input: 100, output: 40, cacheRead: 60, cacheMiss: 40,
  cacheWrite: 20, costUsd: 0.001, unpricedRequests: 0 };
const unpriced = { input: 50, output: 20, cacheRead: 25, cacheMiss: 25,
  cacheWrite: 0, costUsd: null, unpricedRequests: 1 };
const days = [{ date: "2026-11-18", models: { "claude-sonnet-5": priced, "private-model": unpriced } }];

test("cache reads and writes are subsets of input; unpriced usage still counts", () => {
  assert.deepEqual(sumUsage([priced, unpriced]), { input: 150, output: 60, cacheRead: 85,
    cacheMiss: 65, cacheWrite: 20, costUsd: 0.001, unpricedRequests: 1, total: 210 });
  assert.equal(sumUsage([unpriced]).costUsd, null);
  assert.deepEqual(sumUsage([]), { ...emptyCounts(), total: 0 });
});

test("inclusive periods and prior comparisons cross months and leap years correctly", () => {
  assert.deepEqual(usageRange("2024-03-01", "7"), { start: "2024-02-24", end: "2024-03-01" });
  assert.deepEqual(usageRange("2026-01-03", "30"), { start: "2025-12-05", end: "2026-01-03" });
  const leapYear = usageRange("2024-12-31", "year");
  assert.deepEqual(previousUsageRange(leapYear), { start: "2022-12-31", end: "2023-12-31" });
  assert.equal(shiftUsageDate("2024-02-28", 1), "2024-02-29");
  assert.equal(localUsageDate(new Date(2026, 0, 3, 0, 15)), "2026-01-03");
});

test("ledger model filters add up to the combined total and fill inactive dates", () => {
  const range = { start: "2026-11-18", end: "2026-11-19" };
  const all = dailyUsage(days, range, null);
  assert.deepEqual(all[0], { date: "2026-11-18", ...sumUsage([priced, unpriced]) });
  assert.deepEqual(all[1], { date: "2026-11-19", ...sumUsage([]) });
  assert.equal(all[0].total, dailyUsage(days, range, "claude-sonnet-5")[0].total
    + dailyUsage(days, range, "private-model")[0].total);
  assert.equal(dailyUsage(days, range, "not-in-the-ledger")[0].total, 0);
});

test("monthly chart keeps the same tokens and cost as the selected daily range", () => {
  const daily = dailyUsage(days, usageRange("2026-11-19", "year"), null);
  const monthly = usageTrend(daily, true);
  assert.equal(monthly[0].date, "2026-01");
  assert.deepEqual(sumUsage(monthly), sumUsage(daily));
  assert.deepEqual(usageTrend(daily, false), daily);
});
