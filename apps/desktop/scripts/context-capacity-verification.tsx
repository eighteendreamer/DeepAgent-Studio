// Local UI fixture: production component, synthetic snapshots, no model requests.
import React, { useState } from "react";
import { createRoot } from "react-dom/client";
import "../src/styles.css";
import { Button } from "../src/components/shadcn/button";
import { ContextCapacityIndicator } from "../src/components/ContextCapacityIndicator";
import type { ContextUsageSnapshot } from "../src/types";

function Fixture() {
  const [scenario, setScenario] = useState(0);
  const projected = [23_500, 25_000, 8_000][scenario];
  const snapshot: ContextUsageSnapshot = {
    model_id: "deepseek-flash", context_window: 1_000_000,
    prompt_budget: 681_500, estimated_prompt_tokens: projected,
    projected_request_tokens: projected, last_request_prompt_tokens: 23_000,
    measured_prompt_tokens: 23_000, compaction_threshold: 678_464,
    used_ratio: projected / 1_000_000, reserved_output_tokens: 256_000,
    reserved_tool_tokens: 62_500, cache_hit_tokens: 22_000,
    cache_miss_tokens: 1_000, compacted: scenario === 2,
    blocks: [
      { kind: "stable_prefix", name: "稳定系统提示", tokens: 1_000, source: "surface_meter", cache_scope: "stable_prefix" },
      { kind: "tool_schemas", name: "工具定义", tokens: 2_000, source: "surface_meter", cache_scope: "stable_prefix" },
      { kind: "recent_conversation", name: "最近对话", tokens: projected - 3_000, source: "surface_meter", cache_scope: "dynamic" },
    ],
  };
  return <main className="min-h-screen bg-white p-12 text-text-base">
    <h1 className="mb-4 text-xl font-semibold">上下文投影验证（模拟数据）</h1>
    <div className="flex gap-3">
      {["请求前", "输出追加后", "压缩后"].map((label, index) =>
        <Button key={label} variant="outline" onClick={() => setScenario(index)}>{label}</Button>)}
    </div>
    <div className="mt-64 flex max-w-xl justify-end rounded-2xl border border-border-theme p-6">
      <ContextCapacityIndicator snapshot={snapshot} />
    </div>
  </main>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
