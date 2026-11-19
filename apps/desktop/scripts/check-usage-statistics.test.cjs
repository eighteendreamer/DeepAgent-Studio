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
  createDemoUsageDays, shiftUsageDate, localUsageDate } = implementation.exports;

test("cache reads remain part of input and are never added to total twice", () => {
  assert.deepEqual(sumUsage([{ input: 100, output: 20, cacheRead: 80 },
    { input: 50, output: 10, cacheRead: 30 }]), { input: 150, output: 30, cacheRead: 110, total: 180 });
  assert.deepEqual(sumUsage([]), { input: 0, output: 0, cacheRead: 0, total: 0 });
});

test("inclusive periods and prior comparisons cross months and leap years correctly", () => {
  assert.deepEqual(usageRange("2024-03-01", "7"), { start: "2024-02-24", end: "2024-03-01" });
  assert.deepEqual(usageRange("2026-01-03", "30"), { start: "2025-12-05", end: "2026-01-03" });
  const leapYear = usageRange("2024-12-31", "year");
  assert.deepEqual(leapYear, { start: "2024-01-01", end: "2024-12-31" });
  assert.deepEqual(previousUsageRange(leapYear), { start: "2022-12-31", end: "2023-12-31" });
  assert.equal(shiftUsageDate("2024-02-28", 1), "2024-02-29");
  assert.equal(localUsageDate(new Date(2026, 0, 3, 0, 15)), "2026-01-03");
});

test("model filters add up to the combined total and fill inactive dates", () => {
  const days = [{ date: "2026-11-18", models: { chat: { input: 100, output: 40, cacheRead: 60 },
    reasoner: { input: 50, output: 20, cacheRead: 25 } } }];
  const range = { start: "2026-11-18", end: "2026-11-19" };
  const all = dailyUsage(days, range, "all");
  assert.equal(all.length, 2);
  assert.deepEqual(all[0], { date: "2026-11-18", input: 150, output: 60, cacheRead: 85, total: 210 });
  assert.deepEqual(all[1], { date: "2026-11-19", input: 0, output: 0, cacheRead: 0, total: 0 });
  const chat = dailyUsage(days, range, "chat"), reasoner = dailyUsage(days, range, "reasoner");
  assert.equal(all[0].total, chat[0].total + reasoner[0].total);
  assert.equal(chat[0].cacheRead, 60);
});

test("monthly trend aggregation conserves input, output and cache totals", () => {
  const days = createDemoUsageDays("2026-11-19");
  const daily = dailyUsage(days, usageRange("2026-11-19", "year"), "all");
  const monthly = usageTrend(daily, true);
  assert.equal(monthly.length, 11);
  assert.equal(monthly[0].date, "2026-01");
  assert.deepEqual(sumUsage(monthly), sumUsage(daily));
  assert.deepEqual(usageTrend(daily, false), daily);
});

test("demo data is deterministic, has no future rows and covers the previous comparison", () => {
  for (const today of ["2024-12-31", "2026-01-01", "2026-11-19"]) {
    const days = createDemoUsageDays(today);
    assert.deepEqual(days, createDemoUsageDays(today));
    assert.ok(days[0].date <= previousUsageRange(usageRange(today, "year")).start);
    assert.equal(days.at(-1).date, today);
    for (let index = 0; index < days.length; index++) {
      const day = days[index];
      if (index) assert.equal(day.date, shiftUsageDate(days[index - 1].date, 1));
      for (const counts of Object.values(day.models)) {
        assert.ok(counts.input >= 0 && counts.output >= 0);
        assert.ok(counts.cacheRead >= 0 && counts.cacheRead <= counts.input);
      }
    }
  }
});
