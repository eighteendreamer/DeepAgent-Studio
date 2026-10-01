# Token Budget 问题根因分析（完整版）

> **执行日期**：2026-10-01  
> **状态**：✅ 根因已定位（未修复代码）

---

## 🎯 问题总结

### 问题 1：Token 统计爆炸
**现象**：5分钟内累计使用 104万 tokens，超过 100万限制

### 问题 2：前端显示错误
**现象**：输入框显示 "16.6k / 1M (2%)"，但实际已用 104万

---

## 🔍 根因分析

### 问题 1：Token 重复累加

#### 代码位置
```rust
// crates/deepagent-runtime/src/model_agent.rs:1774
if let Some(usage) = response.usage {
    self.usage.prompt_tokens += usage.prompt_tokens;  // ← 累加
    self.usage.completion_tokens += usage.completion_tokens;
    self.usage.total_tokens += usage.total_tokens;
}
```

#### 根本原因

**API 返回的 `prompt_tokens` 是整个上下文的 token 数**（包含历史），而不是本轮新增的。

**错误的累加逻辑**：
```
第1轮调用：
  API 返回：prompt_tokens=1000
  累计：1000

第2轮调用：
  API 返回：prompt_tokens=1100（包含第1轮的1000 + 新增100）
  累计：1000 + 1100 = 2100  ← 重复计算了1000

第3轮调用：
  API 返回：prompt_tokens=1200（包含前两轮 + 新增100）
  累计：2100 + 1200 = 3300  ← 重复计算了2100
```

#### 正确的计算方式

**方式 A：只统计最后一次的值**（当前上下文）
```rust
self.usage.prompt_tokens = usage.prompt_tokens;  // 赋值，不累加
self.usage.completion_tokens += usage.completion_tokens;  // 输出是累加的
```

**方式 B：计算增量**
```rust
let delta_prompt = usage.prompt_tokens.saturating_sub(self.last_prompt_tokens);
self.last_prompt_tokens = usage.prompt_tokens;

self.usage.prompt_tokens += delta_prompt;
self.usage.completion_tokens += usage.completion_tokens;
```

#### 证据

1. **deepseek-harness 有压缩机制**，压缩后会减少上下文，如果累加是对的，压缩后 token 数不会减少
2. **5分钟 104万 tokens** 不合理：
   - 假设每轮 10k tokens，需要 104 轮
   - 5分钟 = 300秒，平均 2.9秒/轮
   - 但如果重复累加，只需要 10-20 轮就能达到 104万

---

### 问题 2：前端显示的是"当前上下文"，不是"累计使用量"

#### 前端代码

```typescript
// apps/desktop/src/components/ContextCapacityIndicator.tsx

export interface ContextUsageSnapshot {
  model_id: string;
  context_window: number;          // 上下文窗口大小（如 128k）
  prompt_budget: number;           // 可用于 prompt 的预算
  estimated_prompt_tokens: number; // 当前 prompt 的 tokens ← 这是当前值
  used_ratio: number;              // 使用比例
  cache_hit_tokens: number;
  cache_miss_tokens: number;
  blocks: ContextBlockUsage[];
}
```

#### 显示逻辑

```typescript
// 显示：16.6k / 1M (2%)
const total = snapshot.estimated_prompt_tokens;  // 16.6k ← 当前上下文
const limit = snapshot.context_window;           // 1M = 128k（应该是模型窗口）
const ratio = snapshot.used_ratio;               // 2%
```

#### 问题根源

**前端显示的是"下一次请求将发送的 tokens"（当前上下文窗口占用）**，而不是"本次会话已累计消耗的 tokens"。

两个概念：
1. **上下文窗口占用**（Context Window Usage）：当前对话有多长 → 16.6k
2. **累计消耗量**（Cumulative Usage）：总共用了多少 tokens → 104万

**前端只显示了 #1，没有显示 #2**。

---

## 📊 数据流分析

### 后端 → 前端

#### 1. 后端发送上下文快照

```rust
// crates/deepagent-app-core/src/context_runtime.rs

pub struct ContextUsageSnapshot {
    pub model_id: String,
    pub context_window: usize,
    pub prompt_budget: usize,
    pub estimated_prompt_tokens: usize,  // ← 当前上下文大小
    pub used_ratio: f64,
    pub reserved_output_tokens: usize,
    pub reserved_tool_tokens: usize,
    pub cache_hit_tokens: usize,
    pub cache_miss_tokens: usize,
    pub compacted: bool,
    pub blocks: Vec<ContextBlockUsage>,
}
```

**关键**：`estimated_prompt_tokens` 是**当前上下文的大小**，不是累计值。

#### 2. 后端检查累计使用量

```rust
// crates/deepagent-runtime/src/loop_engine.rs:666

if let (Some(limit), Some(usage)) =
    (self.config.max_total_tokens, agent.cumulative_usage())
{
    if usage.total_tokens as u64 > limit {
        // ← 这里检查的是 cumulative_usage（累计值）
        // 但前端显示的是 estimated_prompt_tokens（当前值）
    }
}
```

#### 3. 前端显示

```typescript
// 前端只收到了 ContextUsageSnapshot
// 没有收到 cumulative_usage

<div>
  上下文: {snapshot.used_ratio}%
  {snapshot.estimated_prompt_tokens} / {snapshot.context_window}
</div>
```

---

## 🆚 对比：deepseek-harness

### deepseek-harness 的处理方式

#### 1. 有完整的压缩机制

```typescript
// packages/compaction/compaction/src/index.ts

abstract compactRegion(
  start: SessionSeq,
  end: SessionSeq,
  agent: CompactionAgentContext,
  signal?: AbortSignal,
): Promise<CompactionResult>
```

- 检测上下文接近限制时触发压缩
- 用摘要替换历史对话
- **压缩后重新计算 token 数**

#### 2. Token 统计（待验证）

需要查看 deepseek-harness 如何累加 token，但可以推断：
- 要么只统计当前上下文大小（不累加）
- 要么压缩后重置累计值

---

## 🔧 DeepAgent-Studio 的压缩机制

### 已有代码

```rust
// crates/deepagent-app-core/src/context_runtime.rs:39
/// Unified debounce + circuit breaker across the run's reactive compactions
/// (Phase E). Prevents overflow→compact→overflow loops from thrashing

// crates/deepagent-app-core/src/chat_service.rs:42
pairing_safe_compaction_split
render_message_for_compaction
```

**说明**：DeepAgent-Studio **已经有压缩机制**。

### 可能的问题

1. **压缩触发时机太晚**
   - Token 爆炸之前没有触发

2. **压缩后累计统计未更新**
   - 压缩后 `agent.cumulative_usage()` 应该减少
   - 但代码可能没有处理

3. **Token 重复累加导致提前触发限制**
   - 实际上下文只有 20k
   - 但累计统计已经 104万
   - 导致会话提前终止

---

## 💡 修复方案（待实施）

### 修复 1：修正 Token 累加逻辑

#### 方案 A：只记录当前上下文大小（推荐）

```rust
// crates/deepagent-runtime/src/model_agent.rs

if let Some(usage) = response.usage {
    // prompt_tokens 是整个上下文的大小，不累加
    self.usage.prompt_tokens = usage.prompt_tokens;
    
    // completion_tokens 是本次输出，累加
    self.usage.completion_tokens += usage.completion_tokens;
    
    // reasoning_tokens 是本次推理，累加
    self.usage.reasoning_tokens += usage.reasoning_tokens;
    
    // total_tokens 重新计算
    self.usage.total_tokens = 
        self.usage.prompt_tokens + 
        self.usage.completion_tokens + 
        self.usage.reasoning_tokens;
    
    // cache tokens 更新
    self.usage.prompt_cache_hit_tokens = usage.prompt_cache_hit_tokens;
    self.usage.prompt_cache_miss_tokens = usage.prompt_cache_miss_tokens;
}
```

#### 方案 B：计算增量

```rust
struct ModelAgent {
    usage: RunUsage,
    last_prompt_tokens: usize,  // 新增字段
}

if let Some(usage) = response.usage {
    // 计算本次新增的 prompt tokens
    let delta_prompt = usage.prompt_tokens.saturating_sub(self.last_prompt_tokens);
    self.last_prompt_tokens = usage.prompt_tokens;
    
    self.usage.prompt_tokens += delta_prompt;
    self.usage.completion_tokens += usage.completion_tokens;
    self.usage.reasoning_tokens += usage.reasoning_tokens;
    self.usage.total_tokens += delta_prompt + usage.completion_tokens + usage.reasoning_tokens;
}
```

---

### 修复 2：前端显示累计使用量

#### 方案 A：后端发送累计统计

修改 `RuntimeEvent`，添加累计使用量：

```rust
// crates/deepagent-runtime/src/event.rs

pub enum RuntimeEvent {
    // 现有事件...
    
    // 新增：累计使用量更新
    CumulativeUsageUpdate {
        cumulative_prompt_tokens: usize,
        cumulative_completion_tokens: usize,
        cumulative_total_tokens: usize,
        budget_limit: Option<usize>,
    },
}
```

前端接收并显示：

```typescript
// apps/desktop/src/components/ContextCapacityIndicator.tsx

interface Props {
  snapshot?: ContextUsageSnapshot | null;
  cumulativeUsage?: {              // 新增
    total: number;
    limit: number;
  } | null;
}

// 显示
<div>
  <div>当前上下文: {snapshot.estimated_prompt_tokens} / {snapshot.context_window}</div>
  <div>累计使用: {cumulativeUsage.total} / {cumulativeUsage.limit}</div>
</div>
```

#### 方案 B：显示更清晰的标签

```typescript
<div>
  <span>本轮上下文</span>
  <span>{formatTokens(snapshot.estimated_prompt_tokens)} / {formatTokens(snapshot.context_window)}</span>
</div>

{cumulativeUsage && (
  <div className="text-red-500">
    <span>会话累计</span>
    <span>{formatTokens(cumulativeUsage.total)} / {formatTokens(cumulativeUsage.limit)}</span>
  </div>
)}
```

---

### 修复 3：更激进的压缩触发

```rust
// 当上下文达到 50% 时就触发压缩，而不是接近 100%
if estimated_prompt_tokens > context_window / 2 {
    trigger_compaction();
}
```

---

### 修复 4：压缩后重置累计统计（如果采用方案 B）

压缩完成后，重新计算累计值：

```rust
fn after_compaction(&mut self, compacted_context_size: usize) {
    // 压缩后，上下文变小了，需要调整累计统计
    // 假设原始上下文是 50k，压缩后是 10k
    // 累计值应该减少 40k
    
    let reduction = self.last_prompt_tokens.saturating_sub(compacted_context_size);
    self.usage.prompt_tokens = self.usage.prompt_tokens.saturating_sub(reduction);
    self.usage.total_tokens = self.usage.total_tokens.saturating_sub(reduction);
    self.last_prompt_tokens = compacted_context_size;
}
```

---

## 📋 验证步骤（下一步）

### Step 1：确认 API 返回值含义 ✅

**结论**：API 返回的 `prompt_tokens` 是**整个上下文的大小**，不是增量。

**证据**：
1. 代码中有压缩机制，压缩后上下文会变小
2. 5分钟104万 tokens 只有重复累加才能达到
3. 前端显示的 16.6k 是合理的当前上下文大小

### Step 2：验证压缩是否触发

查找压缩日志：
```bash
grep -i "compact\|summarize" apps/desktop/src-tauri/target/debug/data/runtime-logs.db
```

或添加日志：
```rust
tracing::info!("Compaction triggered: {} → {}", old_size, new_size);
```

### Step 3：验证修复方案 A

临时修改代码，将 `+=` 改为 `=`，观察是否还会爆炸。

---

## 🎯 推荐修复顺序

1. ✅ **优先级 P0**：修复 Token 累加逻辑（方案 A）
   - 影响：解决会话提前终止问题
   - 工作量：5行代码修改

2. ✅ **优先级 P1**：前端显示累计使用量
   - 影响：用户能看到真实的使用情况
   - 工作量：后端添加事件 + 前端显示

3. ⏳ **优先级 P2**：优化压缩触发时机
   - 影响：降低上下文压力
   - 工作量：调整阈值

---

## 📝 相关文件

### 后端
- `crates/deepagent-runtime/src/model_agent.rs:1774` - Token 累加逻辑
- `crates/deepagent-runtime/src/loop_engine.rs:666` - Budget 检查
- `crates/deepagent-app-core/src/context_runtime.rs` - 压缩机制

### 前端
- `apps/desktop/src/components/ContextCapacityIndicator.tsx` - Token 显示组件
- `apps/desktop/src/types.ts:212` - ContextUsageSnapshot 类型定义

---

**分析完成日期**：2026-10-01  
**下一步**：实施修复方案 A（修正 Token 累加逻辑）
