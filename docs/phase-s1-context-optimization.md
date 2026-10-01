# Phase S1：上下文优化实施方案

> **优先级**：P0（阻塞生产质量）  
> **预期工期**：2 周  
> **目标**：降低上下文冗余，提升 token 效率，为长对话奠定基础

---

## 1. 背景

当前 DeepAgent 上下文工程存在 3 个 P0/P1 缺陷（已在 `deepagent-defects-and-fixes.md` §2 列出）：

1. **【P0】MCP instructions 全量重发**：每轮对话重新渲染所有 MCP server 的 instructions（可达 10KB+），已宣布的 server 应增量披露
2. **【P1】PromptBudget::fit() 无生产调用**：`assemble(usize::MAX)` 绕过预算检查，溢出时才触发压缩，应主动在预算内构建
3. **【P1】Tokenizer 估算粗、无校准**：用启发式 `4 chars = 1 token`，与 DeepSeek 真实 usage 无对比，误差未量化

这三项导致：
- 长对话提前触发压缩（阈值 70% @ 200k = 140k）
- 用户感知"对话变慢"（压缩耗时 + 更大 prompt）
- 无法充分利用 200k 上下文窗口

---

## 2. 实施项

### 2.1 MCP Instructions 增量披露（P0）

#### 2.1.1 设计

**当前行为**：
```rust
// crates/deepagent-app-core/src/mcp_service.rs:420
pub fn generate_instructions(&self) -> String {
    // 每轮全量渲染所有 server instructions
    servers.iter()
        .map(|s| format!("## {}\n{}", s.name, s.manifest.instructions))
        .collect::<Vec<_>>()
        .join("\n\n")
}
```

**目标行为**（参考 Claude Code `mcpInstructionsDelta.ts`）：
1. 首次渲染：全量 instructions
2. 后续轮次：
   - 已宣布的 server（在 session 历史中出现）→ 仅输出 `## <server_name> (already announced)`
   - 新 server / 新启用的 server → 全量 instructions
3. Session 记录：`declared_mcp_servers: HashSet<String>` 持久化到 `sessions` 表

#### 2.1.2 实施路径

**步骤 1**：在 `Session` 增加 `declared_mcp_servers` 字段

```rust
// crates/deepagent-persistence/src/session.rs
pub struct Session {
    pub id: String,
    pub workspace: PathBuf,
    pub created_at: i64,
    pub last_active_at: i64,
    pub declared_mcp_servers: HashSet<String>, // 新增
}
```

**步骤 2**：在 `SessionStore` 增加更新方法

```rust
// crates/deepagent-persistence/src/session_store.rs
impl SessionStore {
    pub fn mark_mcp_server_declared(
        &self,
        session_id: &str,
        server_name: &str,
    ) -> Result<()> {
        // UPDATE sessions SET declared_mcp_servers = json_array_append(...)
    }
}
```

**步骤 3**：重构 `generate_instructions()` 为 `generate_instructions_delta()`

```rust
// crates/deepagent-app-core/src/mcp_service.rs
impl McpService {
    pub fn generate_instructions_delta(
        &self,
        declared_servers: &HashSet<String>,
    ) -> (String, HashSet<String>) {
        let mut instructions = String::new();
        let mut newly_declared = HashSet::new();
        
        for server in self.list_enabled_servers() {
            if declared_servers.contains(&server.name) {
                // 已宣布 → 简略引用
                instructions.push_str(&format!("## {} (already announced)\n", server.name));
            } else {
                // 新 server → 全量 instructions
                instructions.push_str(&format!("## {}\n{}\n", server.name, server.manifest.instructions));
                newly_declared.insert(server.name.clone());
            }
        }
        
        (instructions, newly_declared)
    }
}
```

**步骤 4**：在 `RunAssembler` 调用 delta 版本

```rust
// crates/deepagent-app-core/src/run_assembler.rs
let session = Session::load_or_create(session_id)?;
let (mcp_instructions, newly_declared) = mcp_service
    .generate_instructions_delta(&session.declared_mcp_servers);

// 更新 session
for server in newly_declared {
    session_store.mark_mcp_server_declared(session_id, &server)?;
}
```

#### 2.1.3 验证

- 单元测试：`mcp_service.rs` 增加 `test_instructions_delta()` — 首次全量、后续简略
- 集成测试：启动真实 MCP server，多轮对话，验证第二轮 instructions 字节数下降

#### 2.1.4 预期收益

假设 3 个 MCP server，每个 instructions 3KB：
- 当前：每轮 9KB
- 优化后：首轮 9KB，后续轮次 <300B（仅 server 名称）
- **节省**：~8.7KB/轮（约 2k tokens）

---

### 2.2 PromptBudget 主动约束（P1）

#### 2.2.1 设计

**当前行为**：
```rust
// crates/deepagent-context/src/assembly.rs:180
pub fn assemble(&self, max_tokens: usize) -> Result<String> {
    // max_tokens 传入 usize::MAX，完全不受预算约束
    let mut prompt = String::new();
    for section in &self.sections {
        prompt.push_str(&section.render());
    }
    prompt
}
```

**目标行为**：
1. `assemble()` 调用 `PromptBudget::fit()`，**主动在预算内**构建 prompt
2. 预算不足时，按优先级裁剪可选 sections：
   - P0（必保）：`[IDENTITY]`, `[RULES]`, `[TASK]`（最后一条用户消息）
   - P1（次要）：`[SKILLS]`（可延迟加载）、`[MCP_TOOLS]`（可 deferred）
   - P2（可丢弃）：历史 turn 的工具结果（超过 5 轮前的）

#### 2.2.2 实施路径

**步骤 1**：定义 section 优先级

```rust
// crates/deepagent-context/src/system_context.rs
pub enum SectionPriority {
    Critical,  // IDENTITY, RULES, TASK
    Important, // SKILLS, MCP_TOOLS
    Optional,  // 历史工具结果
}

pub struct Section {
    pub name: String,
    pub content: String,
    pub priority: SectionPriority,
    pub estimated_tokens: usize,
}
```

**步骤 2**：实现预算约束逻辑

```rust
// crates/deepagent-context/src/prompt_budget.rs
impl PromptBudget {
    pub fn fit_sections(&self, sections: &[Section]) -> Vec<Section> {
        let mut result = Vec::new();
        let mut used = 0;
        
        // 先加入 Critical
        for section in sections.iter().filter(|s| matches!(s.priority, SectionPriority::Critical)) {
            result.push(section.clone());
            used += section.estimated_tokens;
        }
        
        // 按优先级 Important → Optional 逐步加入
        for priority in [SectionPriority::Important, SectionPriority::Optional] {
            for section in sections.iter().filter(|s| matches!(s.priority, priority)) {
                if used + section.estimated_tokens > self.policy.compaction_threshold {
                    // 预算不足，跳过此 section
                    tracing::warn!(
                        section = %section.name,
                        tokens = section.estimated_tokens,
                        "skipped due to budget constraint"
                    );
                    continue;
                }
                result.push(section.clone());
                used += section.estimated_tokens;
            }
        }
        
        result
    }
}
```

**步骤 3**：在 `AssembledPrompt::build()` 调用

```rust
// crates/deepagent-context/src/assembly.rs
pub fn build(context: &SystemContext, policy: ContextPolicy) -> Self {
    let sections = context.build_sections(); // 含优先级
    let budget = PromptBudget::new(policy);
    let fitted_sections = budget.fit_sections(&sections);
    
    Self {
        sections: fitted_sections,
        policy,
    }
}
```

#### 2.2.3 验证

- 单元测试：`prompt_budget.rs` 测试 `fit_sections()` — Critical 必保、Optional 可裁
- 集成测试：构造 150k tokens 的 sections，验证 fit 后 <= 140k（70% 阈值）

#### 2.2.4 预期收益

- 长对话不再"意外溢出" → 避免紧急压缩（耗时 ~500ms）
- 用户感知：对话流畅，无卡顿

---

### 2.3 Tokenizer 校准（P1）

#### 2.3.1 设计

**当前行为**：
```rust
// crates/deepagent-context/src/tokenizer.rs:18
pub fn estimate_tokens(text: &str) -> usize {
    // 启发式：4 chars = 1 token（英文偏差 ~10%，中文偏差 ~30%）
    (text.len() + 3) / 4
}
```

**目标行为**：
1. 调用后对比 DeepSeek API 返回的 `usage.prompt_tokens`，记录偏差
2. 在 `RuntimeLogStore` 新增 `tokenizer_calibration` 表，记录每轮估算 vs 真实
3. 每 100 轮后，计算平均偏差率，调整估算公式

#### 2.3.2 实施路径（轻量级 MVP）

**步骤 1**：在 `RuntimeEvent` 增加 usage 字段

```rust
// crates/deepagent-runtime/src/events.rs
pub enum RuntimeEvent {
    ThinkingCompleted {
        content: String,
        usage: Option<TokenUsage>, // 新增：来自 DeepSeek API
    },
    // ...
}
```

**步骤 2**：在 `ModelAgent::think_streaming()` 填充 usage

```rust
// crates/deepagent-runtime/src/agents/model_agent.rs
async fn think_streaming(&self, context: &ThinkingContext) -> Result<ThinkingResult> {
    let response = self.model.stream(request).await?;
    // ...
    let usage = response.usage; // DeepSeek 返回的真实 usage
    
    Ok(ThinkingResult {
        content,
        tool_calls,
        usage: Some(usage), // 传递到 event
    })
}
```

**步骤 3**：在 `RuntimeLogStore` 记录校准数据

```rust
// crates/deepagent-app-core/src/runtime_log_store.rs
impl RuntimeLogStore {
    pub fn log_tokenizer_calibration(
        &self,
        run_id: &str,
        estimated: usize,
        actual: usize,
    ) -> Result<()> {
        // INSERT INTO tokenizer_calibration (run_id, estimated, actual, error_rate)
        let error_rate = (estimated as f64 - actual as f64) / actual as f64;
        // ...
    }
}
```

**步骤 4**：在 `RunAssembler` 调用 log

```rust
// crates/deepagent-app-core/src/run_assembler.rs
let on_event = move |event: RuntimeEvent| {
    if let RuntimeEvent::ThinkingCompleted { usage: Some(usage), .. } = &event {
        let estimated = tokenizer.estimate_tokens(&prompt);
        runtime_log_store.log_tokenizer_calibration(run_id, estimated, usage.prompt_tokens)?;
    }
    // ...
};
```

#### 2.3.3 验证

- 单元测试：`tokenizer.rs` 测试估算公式（英文/中文/混合文本）
- 数据验证：运行 100 轮对话，查询 `tokenizer_calibration` 表，计算平均误差率

#### 2.3.4 预期收益

- 短期：建立误差度量基线（知道启发式偏差多少）
- 长期：调整公式（如 `text.len() / 3.5`），误差降至 <5%

---

## 3. 实施顺序

按 ROI 排序（收益/成本）：

1. **Week 1, Day 1-3**：2.1 MCP Instructions Delta（P0，高收益/中成本）
2. **Week 1, Day 4-5**：2.2 PromptBudget 主动约束（P1，中收益/中成本）
3. **Week 2, Day 1-2**：2.3 Tokenizer 校准（P1，长期收益/低成本）
4. **Week 2, Day 3-5**：集成测试 + 性能验证 + 文档更新

---

## 4. 验收标准

| 指标 | 当前 | 目标 |
|---|---:|---:|
| MCP instructions 第 2 轮字节数 | 9KB | <500B |
| 长对话触发压缩轮次 | ~15 轮 | >25 轮 |
| Tokenizer 平均误差率 | 未知 | <10% |
| 用户感知卡顿（压缩触发） | 偶现 | 消失 |

---

## 5. 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|---|---|---|---|
| Session schema 变更破坏旧数据 | 中 | 高 | 增加 migration 脚本 |
| fit_sections() 误删 Critical section | 低 | 高 | 单元测试覆盖所有优先级组合 |
| Tokenizer 校准数据量不足 | 中 | 低 | 提供默认公式，校准为增量改进 |

---

**报告完成日期**：2026-10-01  
**所需资源**：1 名开发者全职 2 周  
**前置条件**：无（不依赖其他 Phase）  
**后续 Phase**：S2（harness 装配）需等 S1 完成（否则上下文预算不准）
