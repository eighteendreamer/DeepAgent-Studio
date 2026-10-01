# Token Budget 爆炸问题分析

> **现象**：5分钟内 token 使用量达到 104万，超过 100万限制  
> **显示问题**：输入框显示 "16.6k / 1M (2%)" 但实际已用 104万  
> **日期**：2026-10-01

---

## 🔍 问题 1：Token 统计爆炸

### 1.1 现象

```
run failed: run token budget exceeded: used 1043064 tokens, limit 1000000
```

- **时间**：5分钟
- **使用量**：104.3万 tokens
- **限制**：100万 tokens

### 1.2 代码位置

#### 检测点（loop_engine.rs:666）

```rust
if let (Some(limit), Some(usage)) =
    (self.config.max_total_tokens, agent.cumulative_usage())
{
    if usage.total_tokens as u64 > limit {
        let reason = format!(
            "run token budget exceeded: used {} tokens, limit {limit}",
            usage.total_tokens
        );
        // 终止运行
    }
}
```

#### 累加点（model_agent.rs:1774 和后面）

```rust
// 第一处：max_tokens 截断恢复时
if let Some(usage) = response.usage {
    self.usage.prompt_tokens += usage.prompt_tokens;
    self.usage.completion_tokens += usage.completion_tokens;
    self.usage.reasoning_tokens += usage.reasoning_tokens;
    self.usage.total_tokens += usage.total_tokens;
    self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
    self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
}

// 第二处：正常响应后
if let Some(usage) = response.usage {
    self.usage.prompt_tokens += usage.prompt_tokens;  // ← 累加
    self.usage.completion_tokens += usage.completion_tokens;
    self.usage.reasoning_tokens += usage.reasoning_tokens;
    self.usage.total_tokens += usage.total_tokens;
    self.usage.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
    self.usage.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
}
```

### 1.3 关键问题

**API 返回的 `usage.prompt_tokens` 代表什么？**

#### 假设 A：每次返回的是**本轮新增的 tokens**
- ✅ 累加是正确的
- ✅ 5分钟用 104万 tokens 说明模型疯狂读取了大量文件

#### 假设 B：每次返回的是**整个上下文的 tokens**（包含历史）
- ❌ 累加是错误的，会重复计算
- ❌ 计算方式：
  ```
  第1轮：prompt=1000, 累计=1000
  第2轮：prompt=1100（包含第1轮的1000）, 累计=2100 ← 重复计算了1000
  第3轮：prompt=1200（包含前两轮）, 累计=3300 ← 重复计算了2100
  ```

### 1.4 验证方法

#### 方法 1：查看 API 文档

查看 DeepSeek/Anthropic API 的 `usage.prompt_tokens` 定义：
- 如果是"本次请求的输入 tokens"（**包含历史**） → 假设 B 正确
- 如果是"本次新增的 tokens" → 假设 A 正确

#### 方法 2：查看日志

在 `model_agent.rs` 添加日志：
```rust
tracing::debug!(
    "API usage: prompt={}, completion={}, total={}, cumulative={}",
    usage.prompt_tokens,
    usage.completion_tokens,
    usage.total_tokens,
    self.usage.total_tokens  // 累加后的
);
```

观察 `prompt_tokens` 是否递增。

#### 方法 3：对比 deepseek-harness

查看 deepseek-harness 如何处理 token 统计。

---

## 🔍 问题 2：输入框显示错误

### 2.1 现象

输入框显示：
```
上下文: 2%
16.6k / 1M

工具定义: 10.1k
稳定前缀: 3.3k
动态环境: 2.9k

缓存: 100%
```

但实际已用 104万 tokens。

### 2.2 可能原因

#### 原因 A：前端只显示"当前输入"，不显示"累计使用量"
- 前端显示的 16.6k 是"下一次请求将发送的 tokens"
- 不包括历史已经用掉的 tokens

#### 原因 B：前端数据未同步
- 后端已经累计了 104万
- 前端没有收到更新

#### 原因 C：统计口径不一致
- 前端统计的是"上下文窗口占用"（当前对话长度）
- 后端统计的是"总消耗量"（所有请求累加）

### 2.3 代码位置

需要查找：
1. **前端**：输入框显示 token 统计的组件
2. **后端**：发送 token 统计数据的 API

#### 前端搜索关键词
```
"上下文"
"16.6k"
"缓存"
context.*token
usage.*display
```

#### 后端搜索关键词
```
RuntimeEvent.*usage
emit.*usage
token.*report
```

---

## 🆚 对比：deepseek-harness 的处理方式

### 3.1 Compaction（上下文压缩）

deepseek-harness 有完整的压缩机制：

```typescript
// packages/compaction/compaction/src/index.ts

abstract compactRegion(
  start: SessionSeq,
  end: SessionSeq,
  agent: CompactionAgentContext,
  signal?: AbortSignal,
): Promise<CompactionResult>
```

**工作原理**：
1. 检测上下文窗口接近限制
2. 选择一段历史对话（start → end）
3. 调用 LLM 生成摘要
4. 用摘要替换原始对话
5. 返回压缩后的 token 统计

**效果**：
- 保持上下文窗口在合理范围内
- 长任务不会爆炸

### 3.2 Token 统计方式

需要查看 deepseek-harness 如何累加 token：
- 是否也是简单累加？
- 还是压缩后重新计算？

---

## 📋 DeepAgent-Studio 的压缩机制

### 4.1 已有代码

```rust
// crates/deepagent-app-core/src/context_runtime.rs:39
/// Unified debounce + circuit breaker across the run's reactive compactions
/// (Phase E). Prevents overflow→compact→overflow loops from thrashing: a
/// compactions (token count failed to shrink).

// crates/deepagent-app-core/src/chat_service.rs:42
pairing_safe_compaction_split
render_message_for_compaction
```

**说明**：DeepAgent-Studio **已经有压缩机制**！

### 4.2 可能的问题

1. **压缩触发时机不对**
   - 可能在 token 爆炸之前没有触发压缩

2. **压缩后 token 统计未重置**
   - 压缩后应该重新计算 cumulative_usage
   - 但代码可能没有更新

3. **压缩不够激进**
   - 压缩阈值设置过高
   - 压缩比例太小

---

## 🎯 验证步骤（不改代码）

### Step 1：确认 API 返回的 prompt_tokens 含义

查看 API 文档或在控制台打印：

```rust
// 在 model_agent.rs 添加临时日志
eprintln!("=== API Usage ===");
eprintln!("prompt_tokens: {}", usage.prompt_tokens);
eprintln!("completion_tokens: {}", usage.completion_tokens);
eprintln!("cumulative before: {}", self.usage.total_tokens);
eprintln!("cumulative after: {}", self.usage.total_tokens + usage.total_tokens);
```

### Step 2：检查压缩是否触发

搜索日志中的压缩事件：
```bash
grep -i "compact\|summarize" runtime-logs.db
```

或在代码中搜索压缩触发条件：
```bash
grep -rn "compact.*trigger\|should.*compact" crates/
```

### Step 3：检查前端 token 显示逻辑

查找输入框组件：
```bash
cd apps/desktop/src
grep -rn "16.6k\|上下文\|缓存.*100%" . --include="*.tsx" --include="*.ts"
```

### Step 4：检查前后端数据同步

查找 `RuntimeEvent` 中的 usage 事件：
```bash
grep -rn "RuntimeEvent.*Usage\|emit.*usage" crates/
```

---

## 💡 可能的修复方案（待验证后实施）

### 方案 A：修复 Token 统计（如果是重复累加）

**如果 API 返回的是整个上下文的 tokens**，应该改为：

```rust
// 只统计本轮新增的 tokens
let previous_total = self.usage.total_tokens;
self.usage.prompt_tokens = usage.prompt_tokens;  // 直接赋值，不累加
self.usage.completion_tokens += usage.completion_tokens;  // 输出是累加的
self.usage.total_tokens = usage.total_tokens;  // 直接赋值
```

**或者**：只统计增量

```rust
let delta_completion = usage.completion_tokens;
let delta_prompt = usage.prompt_tokens.saturating_sub(self.last_prompt_tokens);
self.last_prompt_tokens = usage.prompt_tokens;

self.usage.prompt_tokens += delta_prompt;
self.usage.completion_tokens += delta_completion;
self.usage.total_tokens += delta_prompt + delta_completion;
```

### 方案 B：更激进的压缩触发

降低压缩阈值：
```rust
// 当使用量达到 50% 时就触发压缩
if usage.total_tokens > limit / 2 {
    trigger_compaction();
}
```

### 方案 C：压缩后重置累计统计

压缩完成后：
```rust
// 重新计算压缩后的上下文大小
let new_context_tokens = calculate_current_context_tokens();
self.usage.prompt_tokens = new_context_tokens;
```

### 方案 D：修复前端显示

同步后端的累计使用量到前端：
```typescript
// 显示真实的累计使用量
<div>累计使用: {cumulativeTokens} / {limit}</div>
<div>当前上下文: {contextTokens} / {contextLimit}</div>
```

---

## 📊 下一步行动

1. ✅ **验证 API 返回值含义**（查文档或打日志）
2. ✅ **检查压缩触发逻辑**
3. ✅ **定位前端显示代码**
4. ⏳ **确认根因后修复**

---

**分析完成日期**：2026-10-01  
**状态**：待验证 API 返回值含义
