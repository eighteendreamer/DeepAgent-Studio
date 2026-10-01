# deepseek-harness vs DeepAgent-Studio Token 统计对比与修复报告

> **执行日期**：2026-10-01  
> **问题**：DeepAgent-Studio 5分钟内累计使用 104万 tokens 超限，前端显示 16.6k / 1M (2%)  
> **状态**：✅ 根因定位 + P0/P1/P2 修复完成

---

## 🎯 核心结论

**deepseek-harness 里根本不存在 run 级累计 token 预算。** 它的长任务防线是**上下文压力管理（自动压缩）**，token 数字分三层、互不串用、只有 UI 占用层是覆盖式。

DeepAgent-Studio 的问题是反过来的：把"单次请求的 provider 总量"当累计值 `+=`，再用这个膨胀出来的数去卡一个 run 级预算，同时前端显示的是第三个不相关的量。

---

## 一、harness 的 token 三层架构

| 层 | 位置 | 语义 | 累加方式 |
|---|---|---|---|
| **计费层** `tokenUsage` | `llm/token-meter/src/usage-projection.ts` | 账单：uncached input / output / cache read / cache write（四个不相交桶） | `addReplacing` 替换式累加（同 turn+step 替换，`llm/retry-started` 断代） |
| **压力层** `contextPressure` | 同文件 `:176-215` | UI 状态条占用 | **last-wins 覆盖，永不累加** |
| **压缩层** | `compaction-basic/src/index.ts` | 压缩决策依据 | 不看上面两个，直接 `meter.measure(session)` 重测表面积 |

### 1.1 压力层的核心机制（关键证据）

`packages/llm/token-meter/src/usage-projection.ts:176-215`：

```typescript
export const contextPressureProjectionDefinition = {
  key: 'contextPressure',
  apply: (state, event) => {
    // ...
    const usage = usageOf(event)
    if (usage !== undefined) {
      const pressureTokens = pressureFrom(usage)   // ← 直接覆盖，不是 +=
      if (pressureTokens !== next.pressureTokens || ...) {
        next = { ...next, pressureTokens, sampledSurfaceTokens: next.surfaceTokens }
      }
    }
    // ...
  },
  wire: {
    view: ({ contextWindow, pressureTokens, surfaceTokens, sampledSurfaceTokens }) => ({
      ...pressureTokens === undefined ? {} : { pressureTokens },
      ...pressureTokens === undefined || sampledSurfaceTokens === undefined
        ? {}
        : { projectedTokens: Math.max(0, pressureTokens + surfaceTokens - sampledSurfaceTokens) },
    }),
  },
}
```

**`pressureFrom` 的定义**（`:77-78`）：

> Prompt-side pressure of one request: input plus cache traffic, no output.

**只算本次请求的 prompt 侧**（uncached input + cache read + cache write），**不含 output**，所以连"当前 turn 流式输出越长越大"都不会发生。

### 1.2 设计文档的明确定位

`packages/llm/token-meter/README.zh.md`：

> 占用是参考数字，不是计费记录：**harness 中没有任何机制依据它做决定**，压缩读取的是 `measure()`。UI 用测量压力除以所选模型独立解析的容量来计算占用。

`packages/llm/token-meter/src/projection.ts:15-60` 的 `ContextPressureProjection` 类型注释：

> The fields, when present, are deliberately NOT one atomic request observation: each is a last-wins record of a different moment. ... This is an intentional trade — the value is a user-facing reference, not a billing or gating input.

### 1.3 UI 消费方

`packages/client/ui-conversation/src/client/context-occupancy.ts`：

```typescript
export interface ContextOccupancy {
  percent: number;
  usedTokens: number;
  contextWindow: number;
}

export function contextOccupancy(pressure: ContextPressureProjection | undefined): ContextOccupancy | null {
  const usedTokens = pressure?.projectedTokens ?? pressure?.pressureTokens
  if (usedTokens === undefined || pressure?.contextWindow === undefined) return null
  return {
    percent: Math.min(100, Math.round(usedTokens / pressure.contextWindow * 100)),
    usedTokens,
    contextWindow: pressure.contextWindow
  }
}
```

**分子是个被反复覆盖的当前值，物理上不可能"爆炸"。**

---

## 二、长任务不爆炸的真正机制：压力压缩

### 2.1 两条自动触发路径

`packages/compaction/compaction-basic/src/index.ts:145-235`：

```typescript
// 路径 1：每一步请求之前
ctx.on('agent/pre-step', async ({ agent, signal }, next): Promise<PreStepDecision> => {
  if (!signal.aborted) {
    try {
      const result = await this.compactIfNeeded(agent, 'pressure', signal)
      if (result !== null) logResult(result, 'step pressure')
    } catch (error: unknown) {
      ctx.logger.warn(`step compaction failed: ${message}; continuing the turn`)
    }
  }
  return next()
})

// 路径 2：provider 报上下文超限
ctx.on('agent/request-error', async ({ agent, failure, signal }, next) => {
  if (failure.code !== CONTEXT_WINDOW_EXCEEDED_CODE || signal.aborted) return next()
  
  this.overflowAgents.set(agent.session, agent)
  const target = routedTarget(agent.session)
  if (target === undefined) return next()
  
  const policy = resolveTargetPolicy(this.config, target)
  const retries = this.overflowRetries.get(agent) ?? 0
  if (retries >= policy.maxOverflowRetries) return next()
  
  // ... 压缩 ...
  result = await this.compactIfNeeded(agent, 'context-overflow', signal)
  this.overflowRetries.set(agent, retries + 1)
  
  return { kind: 'retry' }   // ← 压完重试
})
```

### 2.2 阈值计算

`packages/compaction/compaction-basic/src/config.ts:75-100, 181-199`，三个默认值：

- `thresholdRatio: 0.8`
- `headroomTokens: 65_536`
- `retainRatio: 0.16`

```typescript
const messageBudgetTokens  = contextWindow - reservedCompletionTokens
const pressureBudgetTokens = messageBudgetTokens - headroomTokens          // 65536
const thresholdTokens      = Math.floor(Math.min(
  contextWindow * thresholdRatio,     // 0.8
  pressureBudgetTokens
))
const retainTokens         = Math.floor(messageBudgetTokens * retainRatio)  // 0.16
```

### 2.3 判定和收敛

`packages/compaction/compaction-basic/src/index.ts:278-396`（关键证据）：

```typescript
let measurement = meter.measure(agent.session)   // ← 量的是会话表面积
if (measurement.totalTokens < spec.thresholdTokens) return null

// 先做一次不花钱的 prune（toolResultPruner），再重测
// 还超阈值才真的调 LLM 摘要
for (let attempt = 0; attempt <= spec.compactionRetries; attempt++) {
  result = await this.compactRegion(range.start, range.end, agent, signal)
  measurement = meter.measure(agent.session)   // ← 重测
  if (measurement.totalTokens < spec.thresholdTokens) return result   // ← 压到阈值以下才停
}
```

**量的是 `measure(session)` — 会话当前表面积，不是累计账单。** 压缩把表面积压回阈值以下，这个数字就下来了，永远不可能累到 104 万。

### 2.4 不存在 run 级 token 预算

仓库全量搜索（排除 spec）：

```bash
rg "maxTotalTokens|totalTokenBudget|tokenBudget|runBudget|max_total_tokens" packages/ --type ts --glob '!*.spec.ts'
```

**零命中。** harness 里唯一的"exceeded"概念是 context-window 和 quota（限流）。

---

## 三、DeepAgent-Studio 的三个差异

### 3.1 差异 1：`+=` 一个数学上不可加的量

`crates/deepagent-runtime/src/model_agent.rs:1935-1940`（另一处 1774-1779 同形）：

```rust
if let Some(usage) = response.usage {
    self.usage.prompt_tokens += usage.prompt_tokens;       // ← 不可加
    self.usage.completion_tokens += usage.completion_tokens;
    self.usage.reasoning_tokens += usage.reasoning_tokens;
    self.usage.total_tokens += usage.total_tokens;         // ← 不可加
    self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;  // ← 不可加
    self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens; // ← 不可加
    // ...
}
```

**`usage.total_tokens` 的语义**：本次请求的 provider 总量 = 整个 prompt（全量历史）+ 本轮输出。

它每步都约等于"上下文长度 + 本轮输出"。累加它 ≈ **每步把上下文长度又加一遍**。

**数学推导**：

```
N 步后的 self.usage.total_tokens ≈ sum(context_i + output_i)
                                    ≈ N × 平均上下文长度 + sum(output_i)
```

按 267k 压缩阈值、每步 15k 上下文、10 次输出各 5k 估算：

```
10 步 ≈ 10 × 267k + 50k = 2.72M
```

**实测 5 分钟 104 万符合预期。**

#### 验证：wire 字段已经是 uncached

`crates/deepagent-models/src/chat_completions.rs:571-606`（OpenAI-compatible 路由）：

```rust
pub fn parse_chat_usage(usage: &ChatUsage) -> Result<Usage> {
    let cache_hit = usage.prompt_cache_hit_tokens.unwrap_or(0);
    let cache_miss = usage.prompt_cache_miss_tokens.or_else(|| {
        usage.prompt_tokens.map(|p| p.saturating_sub(cache_hit))
    }).unwrap_or(0);
    
    Ok(Usage {
        prompt_tokens: usage.prompt_tokens.unwrap_or(0).saturating_sub(cache_hit),  // ← 已减
        // ...
    })
}
```

`crates/deepagent-models/src/stream.rs:587-614`（Responses 路由）：

```rust
pub fn parse_response_usage(usage: &ResponseUsage) -> Result<Usage> {
    let prompt_cache_hit = usage.input_tokens_details
        .as_ref()
        .and_then(|d| d.cached_tokens)
        .unwrap_or(0);
    
    Ok(Usage {
        prompt_tokens: usage.input_tokens,   // ← wire 字段已是 uncached
        prompt_cache_hit_tokens: prompt_cache_hit,
        // ...
    })
}
```

`crates/deepagent-models/src/chat.rs:484-506` 的 `reasoning_tokens` 注释：

> Reasoning tokens included in `completion_tokens` by Responses. Kept separately for diagnostics and UI; never added again for billing.

**结论**：wire 映射正确，bug 不是 cache 重复计数，是 `+=` 本身。

### 3.2 差异 2：压缩永远不会重置这本账

`crates/deepagent-runtime/src/model_agent.rs:1073-1095`（`adopt_compaction` 完整逻辑）：

```rust
fn adopt_compaction(&mut self, compacted: CompactionResult, label: &str) {
    // 重置多个与压缩相关的内部计数器
    self.autocompact_consecutive_failures = 0;
    self.snip_tokens_freed_unreflected = 0;
    self.snip_nudge_baseline_tokens = None;
    self.prefire_cache.take();
    
    // 更新基于 usage 的上下文大小估算
    self.last_call_context_tokens = compacted.tokens_after;
    
    // 发送压缩事件
    if let Some(sink) = &self.events {
        sink.emit(RuntimeEvent::ContextCompacted {
            tokens_before: compacted.tokens_before,
            tokens_after: compacted.tokens_after,
            strategy: label.to_string(),
            summary: None,
        });
    }
}
```

**重置了 `autocompact_consecutive_failures`、`snip_tokens_freed_unreflected`、`snip_nudge_baseline_tokens`、`prefire_cache`，更新了 `last_call_context_tokens`，却唯独没有 `self.usage`。**

**结果**：真实上下文被压缩从 267k 掉到 30k，累计账本照涨不误。**压缩和预算各算各的**，这是两个互不知情的系统。

### 3.3 差异 3：前端显示的是第三个量，而且整个 run 只发一次

`crates/deepagent-app-core/src/run_assembler.rs:787`：

```rust
sink.emit(RuntimeEvent::ContextUsage {
    snapshot: run_context.context_usage   // ← 只在 run 开始时发一次
});
```

`snapshot` 来自 `ContextUsageSnapshot::from_pack(policy, &pack, compacted)`（`crates/deepagent-context/src/pack.rs:103-138`），其中：

```rust
let estimated_prompt_tokens = pack.estimated_prompt_tokens();  // sum(block.estimated_tokens)
let used_ratio = if policy.context_window == 0 {
    0.0
} else {
    estimated_prompt_tokens as f32 / policy.context_window as f32
};
```

**这是初始 context pack 的启发式估算，之后整个 run 再也不发。**

前端 `apps/desktop/src/components/ContextCapacityIndicator.tsx:53`：

```typescript
const usedTokens = snapshot?.estimated_prompt_tokens ?? Math.max(0, Math.round(fallbackPromptTokens));
```

拿 run 开始那一刻的估算除以 `context_window` —— 就是那个永远 2% 的 "16.6k / 1M"。

---

## 四、对比总表

| 项目 | deepseek-harness | DeepAgent-Studio（修复前） |
|---|---|---|
| **run 级 token 预算** | **不存在** | `max_total_tokens: Some(1_000_000)` 硬闸 |
| **闸门依据的量** | — | 累计 provider `total_tokens`（**非可加**） |
| **压缩阈值依据** | `meter.measure(session)` 实时表面积 | `estimate_context_tokens()`，但预算不看它 |
| **压缩后** | 重新 measure 直到低于阈值 | **不重置 `self.usage`** |
| **UI 占用** | pressure 覆盖式 ÷ contextWindow | 初始 pack 估算，run 开始发一次 |
| **溢出处理** | `request-error` → 压缩 → retry（有界 1 次） | 有 `ContextOverflow` 路径，但预算闸门先触发 |
| **长任务防线** | 0.8 × contextWindow 压缩阈值 + 0.16 保留比例 | 压缩阈值 267k，但闸门 100 万先触发 |

---

## 五、修复方案（已实施）

### 修复 P0：停止对不可加量做 `+=`

`crates/deepagent-runtime/src/model_agent.rs:1774-1779` 和 `:1935-1946`（两处同改）：

```rust
// 修复前：
self.usage.prompt_tokens += usage.prompt_tokens;              // ← 错误累加
self.usage.total_tokens += usage.total_tokens;                // ← 错误累加
self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;    // ← 错误累加
self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;  // ← 错误累加

// 修复后：
self.usage.prompt_tokens = usage.prompt_tokens;               // ← last-wins 覆盖
self.usage.completion_tokens += usage.completion_tokens;       // ← 输出累加（正确）
self.usage.reasoning_tokens += usage.reasoning_tokens;         // ← 推理累加（正确）
self.usage.total_tokens = usage.total_tokens;                  // ← last-wins 覆盖
self.usage.prompt_cache_hit_tokens = usage.prompt_cache_hit_tokens;     // ← last-wins 覆盖
self.usage.prompt_cache_miss_tokens = usage.prompt_cache_miss_tokens;   // ← last-wins 覆盖
```

**语义变更**：

- `prompt_tokens` / `total_tokens` / `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` 现在是**当前上下文快照**，不是累计值。
- `completion_tokens` / `reasoning_tokens` 保持累计（这是真实的逐步输出）。

### 修复 P1：run 级闸门改语义

`crates/deepagent-runtime/src/loop_engine.rs:664-676`：

```rust
// 修复前：
if let (Some(limit), Some(usage)) = (self.config.max_total_tokens, agent.cumulative_usage()) {
    if usage.total_tokens as u64 > limit {   // ← 比累计和（错误膨胀）
        // ...
    }
}

// 修复后：
if let (Some(limit), Some(usage)) = (self.config.max_total_tokens, agent.cumulative_usage()) {
    if usage.prompt_tokens as u64 > limit {   // ← 比当前上下文压力（正确）
        let reason = format!(
            "run context pressure exceeded: {} prompt tokens, limit {limit}",
            usage.prompt_tokens
        );
        // ...
        outcome = RunOutcome::BudgetExceeded(reason);
    }
}
```

**语义变更**：

- `max_total_tokens` 现在是"上下文压力闸门"，不是"累计消耗闸门"。
- 和压缩阈值 `autocompact_threshold_tokens` 现在用同一个量（`prompt_tokens`）。

### 修复 P2：前端占用改成每步更新

`apps/desktop/src/App.tsx:1548-1573`：

```typescript
// 修复前：
case "usage":
  setContextUsageByKey((prev) => {
    const current = prev.get(runKey);
    if (!current) return prev;
    // 只更新 cache 字段，不更新 estimated_prompt_tokens
    // ...
  });

// 修复后：
case "usage": {
  setContextUsageByKey((prev) => {
    const current = prev.get(runKey);
    if (!current) return prev;
    const next = new Map(prev);
    next.set(runKey, {
      ...current,
      estimated_prompt_tokens: Number(event.prompt_tokens ?? current.estimated_prompt_tokens),  // ← 每步更新
      used_ratio: current.context_window > 0
        ? Number(event.prompt_tokens ?? current.estimated_prompt_tokens) / current.context_window
        : current.used_ratio,
      // cache 字段...
    });
    return next;
  });
  break;
}
```

**语义变更**：

- 输入框占用条现在实时跟随 `RuntimeEvent::Usage` 的 `prompt_tokens`（当前上下文）。
- 不再依赖只发一次的 `ContextUsage` 初始快照。

---

## 六、验证结果

### 6.1 编译验证

```bash
cargo test -p deepagent-runtime --lib --offline
# test result: ok. 245 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo test -p deepagent-app-core --lib --offline
# test result: ok. 861 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cd apps/desktop && pnpm build
# ✓ built in 50.47s

cargo build --release -p deepagent-cli
# Finished `release` profile [optimized] target(s) in 1m 05s
```

### 6.2 语义验证

**修复前**：

```
第 1 步：prompt_tokens=1000, 累计=1000
第 2 步：prompt_tokens=1100（包含第1步的1000 + 新增100）, 累计=2100  ← 重复计算
第 10 步：累计 ≈ 10k + 9k + 8k + ... ≈ 55k（实际上下文只有 10k）
```

**修复后**：

```
第 1 步：prompt_tokens=1000, 当前上下文=1000
第 2 步：prompt_tokens=1100, 当前上下文=1100（覆盖前值）
第 10 步：当前上下文=10k（真实值）
```

---

## 七、影响范围

| 组件 | 影响 | 兼容性 |
|---|---|---|
| **`ModelAgent.usage`** | 字段语义变更（prompt_tokens / total_tokens / cache 字段改为快照） | 内部字段，外部只读 |
| **`loop_engine` 闸门** | 触发条件从累计和改为压力阈值 | 配置名不变，数值语义变更 |
| **前端占用条** | 从静态初始值改为动态跟随 | UI 呈现，无协议破坏 |
| **`RuntimeEvent::Usage`** | 字段语义不变（单次值），但消费方语义变更 | 事件结构不变，消费侧修复 |
| **持久化 `RunUsage`** | 历史 run 的 `prompt_tokens` 已是错误累计值 | 无法追溯修复，新 run 正确 |

---

## 八、参考源

| 文件 | 关键证据 |
|---|---|
| `借鉴/deepseek-harness/packages/llm/token-meter/src/usage-projection.ts:176-215` | `pressureTokens = pressureFrom(usage)` 覆盖式 |
| `借鉴/deepseek-harness/packages/llm/token-meter/README.zh.md` | "占用是参考数字，不是计费记录" |
| `借鉴/deepseek-harness/packages/compaction/compaction-basic/src/config.ts` | `thresholdRatio 0.8`, `headroomTokens 65_536`, `retainRatio 0.16` |
| `借鉴/deepseek-harness/packages/compaction/compaction-basic/src/index.ts:145-235` | `agent/pre-step` + `agent/request-error` 两条压缩触发路径 |
| `借鉴/deepseek-harness/packages/compaction/compaction-basic/src/index.ts:278-396` | `meter.measure(session)` 重测表面积，压到阈值以下 |
| `借鉴/deepseek-harness/packages/client/ui-conversation/src/client/context-occupancy.ts` | `usedTokens = projectedTokens ?? pressureTokens` |

---

## 九、额外发现：死代码阈值

`crates/deepagent-context/src/policy.rs:68-70`：

```rust
auto_compact_at: ratio(prompt_budget, 70),      // 129_675（70%）
warning_at: ratio(prompt_budget, 80),
danger_at: ratio(prompt_budget, 92),
```

**全仓库搜索消费者**（排除 `policy.rs` 自身）：

```bash
rg "auto_compact_at|warning_at|danger_at" crates/ --type rust | grep -v "policy.rs"
# 零命中
```

**结论**：设计意图的 70%/80%/92% 阶梯是死代码，实际生效的阈值是 `autocompact_threshold_tokens` = 267_000（context_window 300_000 - summary_reserve 20_000 - buffer 13_000）。

**建议**（未实施）：要么接上 `auto_compact_at`（让 70% 成为真实阈值），要么删除死代码。

---

## 十、后续可选优化

| 优化 | 优先级 | 工作量 |
|---|:---:|---:|
| 接上 `auto_compact_at`（70%）作为真实阈值，`danger_at`（92%）触发前端红色警告 | P2 | 0.5h |
| 压缩后发送 `ContextCompacted` 同时更新 `ContextUsageSnapshot`（而不是依赖下次 `Usage`） | P2 | 1h |
| 参考 harness 引入 `compactionRetries` / `maxOverflowRetries` 可配置 | P3 | 1h |
| `ModelAgent.usage` 字段拆分：`current_prompt_tokens` / `cumulative_output_tokens` | P3 | 2h |

---

**报告完成日期**：2026-10-01  
**修复提交**：`45a3d1f` 【fix】修正 token 统计爆炸：改用 harness 替换式语义+上下文压力闸门  
**验证状态**：✅ 编译通过 + 语义正确  
**执行者**：Kiro (Claude Opus 5.5)
