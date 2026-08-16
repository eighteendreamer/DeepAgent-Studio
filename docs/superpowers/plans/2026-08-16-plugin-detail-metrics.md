# Plugin Detail Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the oversized plugin-detail metric grid with six fixed `102 × 56px` cards that remain compact and wrap naturally.

**Architecture:** Keep the existing `Plugin` DTO and `Metric` component boundary. Change only the metric container, labels, and card typography; add a source-level regression check to the existing dependency-free Node test setup.

**Tech Stack:** React, TypeScript, Tailwind CSS, Node.js built-in test runner, pnpm.

---

### Task 1: Compact capability metric cards

**Files:**
- Create: `apps/desktop/scripts/check-plugin-detail-metrics.test.cjs`
- Modify: `apps/desktop/package.json`
- Modify: `apps/desktop/src/components/PluginsViewReal.tsx:846-853`
- Modify: `apps/desktop/src/components/PluginsViewReal.tsx:1764-1770`

- [x] **Step 1: Write the failing layout regression test**

Create a Node test that reads `PluginsViewReal.tsx` and asserts the metric region uses `flex flex-wrap gap-2`, all six Chinese labels, and cards with `h-14 w-[102px] shrink-0 rounded-md`.

```js
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const test = require("node:test");

const source = readFileSync(
  join(__dirname, "..", "src", "components", "PluginsViewReal.tsx"),
  "utf8",
);

test("plugin detail metrics use fixed compact cards", () => {
  assert.match(source, /className="mb-8 flex flex-wrap gap-2"/);
  for (const label of ["技能", "MCP", "钩子", "命令", "应用", "样式"]) {
    assert.match(source, new RegExp(`Metric label="${label}"`));
  }
  assert.match(source, /h-14 w-\[102px\] shrink-0 rounded-md/);
});
```

- [x] **Step 2: Run the test and verify the current layout fails**

Run: `cd apps/desktop && node --test scripts/check-plugin-detail-metrics.test.cjs`

Expected: FAIL because the current region uses `grid-cols-2 md:grid-cols-5`, English labels, and fluid-width cards.

- [x] **Step 3: Implement the selected B layout**

Change the metric region to:

```tsx
<div className="mb-8 flex flex-wrap gap-2">
  <Metric label="技能" value={plugin.skill_count} />
  <Metric label="MCP" value={plugin.mcp_server_count} />
  <Metric label="钩子" value={plugin.hook_count} />
  <Metric label="命令" value={plugin.command_count} />
  <Metric label="应用" value={plugin.app_count} />
  <Metric label="样式" value={plugin.output_style_count ?? 0} />
</div>
```

Change `Metric` to:

```tsx
function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div className="h-14 w-[102px] shrink-0 rounded-md border border-border-theme px-2.5 py-2">
      <div className="text-[10px] leading-none text-text-secondary">{label}</div>
      <div className="mt-1 text-base font-semibold leading-none text-text-base">{value}</div>
    </div>
  );
}
```

Add `test:plugin-detail` to `package.json` and run it before the existing plugin-page and plugin-toggle checks in `pnpm build`.

- [x] **Step 4: Run focused and production verification**

Run: `cd apps/desktop && pnpm test:plugin-detail`

Expected: one passing test.

Run: `cd apps/desktop && pnpm build`

Expected: metric, page-lifecycle, and toggle tests pass; TypeScript and Vite production build exit successfully.

- [x] **Step 5: Commit the implementation**

```text
git add apps/desktop/package.json apps/desktop/scripts/check-plugin-detail-metrics.test.cjs apps/desktop/src/components/PluginsViewReal.tsx docs/superpowers/plans/2026-08-16-plugin-detail-metrics.md
git commit -m "【fix】缩小插件详情能力卡片"
```
