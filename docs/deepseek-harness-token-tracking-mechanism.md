# deepseek-harness Token 统计机制分析

> **分析日期**：2026-10-01  
> **结论**：✅ deepseek-harness **不累加** API 返回的 prompt_tokens，而是用"替换式累加"避免重复计数

---

## 🎯 核心机制：Replacement-based Accumulation（替换式累加）

### 关键代码

```typescript
// packages/llm/token-meter/src/usage-projection.ts:34-43

const addReplacing = (
  totals: TokenUsageProjection,
  previous: TokenUsageProjection | undefined,
  next: TokenUsageProjection,
): TokenUsageProjection => ({
  uncachedInputTokens: totals.uncachedInputTokens - (previous?.uncachedInputTokens ?? 0) + next.uncachedInputTokens,
  outputTokens: totals.outputTokens - (previous?.outputTokens ?? 0) + next.outputTokens,
  cacheReadTokens: totals.cacheReadTokens - (previous?.cacheReadTokens ?? 0) + next.cacheReadTokens,
  cacheWriteTokens: totals.cacheWriteTokens - (previous?.cacheWriteTokens ?? 0) + next.cacheWriteTokens,
})
```

### 工作原理

**公式**：
```
新总数 = 旧总数 - 上次同一 turn/step 的值 + 本次新值
```

**示例**：

#### 场景1：正常调用（每次都是新 turn）
```
Turn 1, Step 1:
  API 返回：inputTokens=1000, outputTokens=100
  previous = undefined
  totals = 0 - 0 + 1000 = 1000（输入）
  totals = 0 - 0 + 100 = 100（输出）

Turn 2, Step 1:
  API 返回：inputTokens=1100（包含历史1000 + 新增100）, outputTokens=150
  previous = undefined（不同 turn）
  totals = 1000 - 0 + 1100 = 2100（输入） ← 看起来重复了？
```

**等等，这里有个关键点！**

---

## 🔍 关键发现：inputTokens 是什么？

### API 返回值的含义

查看注释：
```typescript
// packages/llm/llm/src/types.ts

/**
 * Counts are DISJOINT: `inputTokens` is uncached input only; cached input is
 * reported separately as `cacheReadTokens`/`cacheWriteTokens` (billed input =
 * sum of the three). Adapters whose providers fold cache hits into a total
 * prompt count (DeepSeek's `prompt_tokens`) subtract them out.
 */
export interface TokenUsage {
  inputTokens: number       // ← 只统计 uncached 的输入！
  outputTokens: number
  totalTokens?: number
  cacheReadTokens?: number  // ← cached 部分单独统计
  cacheWriteTokens?: number
  reasoningTokens?: number
}
```

**重要**：
- `inputTokens` 是 **本次请求新增的 uncached tokens**
- `cacheReadTokens` 是从缓存读取的部分
- **不是整个上下文的大小**

### DeepSeek 适配器的处理

文档说明：
> Adapters whose providers fold cache hits into a total prompt count (DeepSeek's `prompt_tokens`) subtract them out.

**含义**：
- DeepSeek API 返回的 `prompt_tokens` = uncached + cached
- 适配器会**减去** cached 部分，只保留 uncached
- 所以 `inputTokens` 每次都是**增量**，不是累计值

---

## 🆚 对比：DeepAgent-Studio 的问题

### DeepAgent-Studio 的代码

```rust
// crates/deepagent-runtime/src/model_agent.rs:1774

if let Some(usage) = response.usage {
    self.usage.prompt_tokens += usage.prompt_tokens;  // ← 直接累加
    self.usage.completion_tokens += usage.completion_tokens;
    self.usage.total_tokens += usage.total_tokens;
}
```

### 问题分析

#### 如果 DeepSeek API 返回的是"整个上下文大小"

**DeepAgent-Studio 的累加逻辑是错的**：
```
Turn 1: prompt_tokens=1000 → 累计=1000
Turn 2: prompt_tokens=1100 → 累计=2100（重复计算了1000）
```

#### 如果 DeepSeek API 返回的是"增量"

**DeepAgent-Studio 的累加逻辑是对的**，但那为什么会爆炸到104万？

---

## 🔬 验证：DeepSeek API 到底返回什么？

### 需要验证的问题

1. **DeepSeek Chat API 的 `prompt_tokens` 是累计值还是增量？**
   - 如果是累计值 → DeepAgent-Studio 不能直接 `+=`
   - 如果是增量 → DeepAgent-Studio 的逻辑是对的

2. **deepseek-harness 的适配器是否做了"减去 cached"的处理？**
   - 需要查看 DeepSeek 适配器的源码

### 下一步验证

```bash
# 查找 DeepSeek 适配器
find packages -name "*deepseek*.ts" | grep -v node_modules
```

---

## 🔄 "替换式累加"的真实用途

### 真实场景：同一个 turn/step 的重试或流式更新

```typescript
// packages/llm/token-meter/src/usage-projection.ts:122-147

apply: (state, event) => {
    if (event.type === 'llm/retry-started') {
      // 重试时，清空 last，这样下次调用会累加而不是替换
      return state.last?.turn === event.data.turn && state.last.step === event.data.step
        ? { ...state, last: null }
        : state
    }
    
    const { turn, step } = event.data
    const buckets = bucketsFrom(usage)
    
    // 关键：如果是同一个 turn/step 的更新，就替换（不累加）
    const previous = state.last !== null
      && state.last.turn === turn
      && state.last.step === step
      ? state.last.buckets  // ← 找到上次的值
      : undefined           // ← 不同 turn，不替换
    
    return {
      totals: addReplacing(state.totals, previous, buckets),  // ← 执行替换式累加
      last: { turn, step, buckets },  // ← 记住本次的值
    }
  },
```

### 使用场景

**场景1：流式更新**
```
Turn 1, Step 1, chunk 1: inputTokens=1000, outputTokens=50
  previous = undefined
  totals = 0 - 0 + 1000 = 1000

Turn 1, Step 1, chunk 2: inputTokens=1000, outputTokens=100（输出增加了）
  previous = {inputTokens:1000, outputTokens:50}（同一 turn/step）
  totals.input = 1000 - 1000 + 1000 = 1000（没变，符合预期）
  totals.output = 50 - 50 + 100 = 100（更新为新值）
```

**场景2：重试**
```
Turn 1, Step 1, attempt 1: inputTokens=1000, outputTokens=100（失败）
  totals = 1000

llm/retry-started event（turn=1, step=1）
  清空 last = null

Turn 1, Step 1, attempt 2: inputTokens=1000, outputTokens=120（成功）
  previous = undefined（因为 last 被清空了）
  totals = 1000 - 0 + 1000 = 2000  ← 累加了，因为是重试
```

---

## 💡 核心结论

### deepseek-harness 的设计

1. **API 返回的是增量**（或者适配器把累计值转成了增量）
2. **替换式累加的目的**：处理同一 turn/step 的多次更新（流式 / 重试）
3. **不同 turn 之间**：正常累加（因为 `previous = undefined`）

### DeepAgent-Studio 的问题

如果 DeepSeek API 返回的是**累计值**（整个上下文大小），那么：
- **DeepAgent-Studio 的 `+=` 是错的**
- **应该改成 `=`**（赋值，不累加）

或者学习 deepseek-harness，引入"替换式累加"机制。

---

## 🎯 下一步行动

### 1. 验证 DeepSeek API 的行为

**方法 A：看官方文档**
```
https://api-docs.deepseek.com/
```

**方法 B：看 deepseek-harness 的适配器代码**
```bash
find packages -name "*deepseek*.ts" | head -10
```

**方法 C：打印日志验证**
```rust
tracing::info!("API response: prompt_tokens={}, completion_tokens={}", 
    usage.prompt_tokens, usage.completion_tokens);
```

运行两轮，对比：
- 如果第2轮的 `prompt_tokens` > 第1轮 → 是累计值
- 如果第2轮的 `prompt_tokens` 和第1轮无关 → 是增量

### 2. 确定修复方案

#### 方案 A：如果 API 返回累计值
```rust
// 只记录最后一次的值，不累加
self.usage.prompt_tokens = usage.prompt_tokens;
self.usage.completion_tokens += usage.completion_tokens;
```

#### 方案 B：引入"替换式累加"
```rust
// 类似 deepseek-harness
if self.last_turn == current_turn && self.last_step == current_step {
    // 同一 turn/step，替换
    let delta_prompt = usage.prompt_tokens.saturating_sub(self.last_prompt_tokens);
    self.usage.prompt_tokens += delta_prompt;
} else {
    // 不同 turn，累加
    self.usage.prompt_tokens += usage.prompt_tokens;
}
self.last_turn = current_turn;
self.last_step = current_step;
self.last_prompt_tokens = usage.prompt_tokens;
```

#### 方案 C：适配器层面处理
```rust
// 在 DeepSeek 适配器中，把累计值转成增量
impl ModelAdapter for DeepSeekAdapter {
    fn parse_usage(&mut self, response: &ApiResponse) -> Usage {
        let delta_prompt = response.prompt_tokens.saturating_sub(self.last_prompt_tokens);
        self.last_prompt_tokens = response.prompt_tokens;
        
        Usage {
            prompt_tokens: delta_prompt,  // ← 返回增量
            completion_tokens: response.completion_tokens,
            ...
        }
    }
}
```

---

## 📋 总结

| 项目 | deepseek-harness | DeepAgent-Studio |
|------|------------------|------------------|
| **统计方式** | 替换式累加 | 直接累加 `+=` |
| **处理流式** | ✅ 同 turn/step 替换 | ❌ 会重复计算 |
| **处理重试** | ✅ 清空 last 后累加 | ❓ 未知 |
| **API 假设** | 增量或转成增量 | 未处理累计值 |
| **风险** | 低 | **高（会爆炸）** |

**推荐**：
1. 先验证 DeepSeek API 返回的是累计值还是增量
2. 如果是累计值，改成方案 A（`=` 赋值）
3. 如果要支持流式更新，引入方案 B（替换式累加）

---

**分析完成日期**：2026-10-01  
**下一步**：查找 deepseek-harness 的 DeepSeek 适配器源码
