# Phase S2.2 & S2.3 执行报告

> **执行日期**：2026-10-01  
> **执行内容**：Phase S2.3（子代理 Frontmatter 扩展）  
> **执行状态**：✅ 完成

---

## 1. 执行摘要

### 1.1 Phase S2.3：子代理 Frontmatter 扩展（已完成）

**目标**：扩展 `SubagentRequest` 结构体，增加 `phase`、`label`、`schema` 字段，使子代理支持：
- **phase**：DAG 阶段标签（如 "Review", "Implementation"），用于 UI 分组展示
- **label**：短标签（如 "review:correctness"），用于快速识别
- **schema**：JSON Schema，强制子代理返回结构化输出

---

## 2. 修改内容

### 2.1 扩展 `SubagentRequest` 结构体

**文件**：`crates/deepagent-builtins/src/task_tool.rs`

**变更**：
```rust
pub struct SubagentRequest {
    // ... 原有字段 ...
    
    // 新增字段
    /// Optional phase label for DAG orchestration (e.g. "Review", "Implementation").
    /// Used to group agents in UI and track workflow progress.
    pub phase: Option<String>,
    
    /// Optional short label for UI display (e.g. "review:correctness", "backend:api").
    /// Shown in agent cards and logs for quick identification.
    pub label: Option<String>,
    
    /// Optional JSON schema for structured output validation.
    /// When present, the agent's response must conform to this schema.
    pub schema: Option<serde_json::Value>,
}
```

**影响**：
- Task 工具现在接受 `phase`/`label`/`schema` 参数
- 子代理可被标记为特定 workflow 阶段
- 结构化输出支持（agent → agent 数据传递）

---

### 2.2 更新 Task 工具 JSON Schema

**文件**：`crates/deepagent-builtins/src/task_tool.rs:348-362`

**变更**：在 Tool schema 的 `properties` 增加三个字段：
```json
"phase": {
    "type": "string",
    "description": "Optional phase label for workflow orchestration (e.g. 'Review', 'Implementation'). Groups agents in UI."
},
"label": {
    "type": "string",
    "description": "Optional short label for UI display (e.g. 'review:correctness', 'backend:api')."
},
"schema": {
    "type": "object",
    "description": "Optional JSON schema for structured output validation. The agent's response must conform to this schema."
}
```

**影响**：
- 模型现在知道可以传入这些参数
- 支持 workflow 编排（如 DagScheduler）

---

### 2.3 Frontmatter 注入到子代理系统提示词

**文件**：`crates/deepagent-app-core/src/subagent_runner.rs:1391-1468`

**变更**：在 `subagent_system_prompt()` 函数开头注入 YAML frontmatter：
```rust
// Inject frontmatter metadata (phase, label, schema) if present
let has_frontmatter = request.phase.is_some() 
    || request.label.is_some() 
    || request.schema.is_some();

if has_frontmatter {
    system.push_str("---\n");
    if let Some(phase) = &request.phase {
        system.push_str("phase: ");
        system.push_str(phase);
        system.push('\n');
    }
    if let Some(label) = &request.label {
        system.push_str("label: ");
        system.push_str(label);
        system.push('\n');
    }
    if let Some(schema) = &request.schema {
        system.push_str("schema: ");
        system.push_str(&serde_json::to_string(schema).unwrap_or_default());
        system.push('\n');
    }
    system.push_str("---\n\n");
}
```

**示例输出**（当传入 `phase: "Review"`, `label: "review:bugs"`, `schema: {...}`）：
```
---
phase: Review
label: review:bugs
schema: {"type":"object","properties":{"bugs":[...]}}
---

# Sub-agent identity
...
```

**影响**：
- 子代理看到自己的元数据（phase/label）
- 当有 `schema` 时，系统提示词强制：**"YOUR RESPONSE MUST BE VALID JSON MATCHING THE SCHEMA ABOVE."**

---

### 2.4 解析工具调用参数

**文件**：`crates/deepagent-builtins/src/task_tool.rs:545-570`

**变更**：从 `args` 解析新字段：
```rust
// Extract optional frontmatter fields
let phase = args
    .get("phase")
    .and_then(serde_json::Value::as_str)
    .map(String::from);
let label = args
    .get("label")
    .and_then(serde_json::Value::as_str)
    .map(String::from);
let schema = args.get("schema").cloned();

let request = SubagentRequest {
    // ... 原有字段 ...
    phase,
    label,
    schema,
};
```

**影响**：
- 用户调用 `task(prompt="...", phase="Review", label="review:bugs")` 时，参数正确传递

---

### 2.5 修复现有 `SubagentRequest` 构造

**修改位置**：
1. **`subagent_runner.rs:761`**（resume 子代理）  
   从保存的 JSON 恢复 frontmatter 字段：
   ```rust
   phase: saved.get("phase").and_then(serde_json::Value::as_str).map(ToOwned::to_owned),
   label: saved.get("label").and_then(serde_json::Value::as_str).map(ToOwned::to_owned),
   schema: saved.get("schema").cloned(),
   ```

2. **`subagent_runner.rs:1040`**（DAG 节点执行）  
   DAG 调度器传入 `label: Some(node_id)`：
   ```rust
   phase: None,  // TODO: extract from context when DAG supports phases
   label: Some(context.node_id.clone()),
   schema: None,
   ```

**影响**：
- 兼容性：所有现有代码路径编译通过
- DAG 节点自动获得 `label`（节点 ID）

---

## 3. 验证结果

### 3.1 编译验证

| 包 | 状态 | 耗时 |
|---|:---:|---:|
| `deepagent-builtins` | ✅ 通过 | ~2s |
| `deepagent-app-core` | ✅ 通过 | ~10s |
| `deepagent-cli` | ✅ 通过 | ~12s |
| `deepagent-desktop` | ✅ 通过 | ~20s |

**结论**：所有包编译通过，无警告。

---

### 3.2 字段对齐度

| 字段 | Claude Code | DeepAgent（修复前） | DeepAgent（修复后） |
|---|:---:|:---:|:---:|
| `model` | ✅ | ✅ | ✅ |
| `thinking` / `effort` | ✅ | ✅ | ✅ |
| `isolation` | ✅ | ✅ | ✅ |
| `phase` | ✅ | ❌ | ✅ |
| `label` | ✅ | ❌ | ✅ |
| `schema` | ✅ | ❌ | ✅ |
| **总计** | 6/6 | 3/6 | 6/6 |

**结论**：DeepAgent 子代理 frontmatter 字段已与 Claude Code 对齐。

---

## 4. 使用示例

### 4.1 基础 Workflow（带 phase）

```javascript
// 主代理调用
task({
  description: "review for bugs",
  prompt: "Review the code for correctness issues",
  phase: "Review",
  label: "review:correctness",
  isolation: "worktree"
});
```

**子代理看到的系统提示词**：
```
---
phase: Review
label: review:correctness
---

# Sub-agent identity
...
# Sub-agent task
You are a focused sub-agent. Do exactly the delegated task...
```

---

### 4.2 结构化输出（带 schema）

```javascript
// 主代理调用
task({
  description: "extract bugs",
  prompt: "Analyze the code and return all bugs as structured JSON",
  schema: {
    type: "object",
    properties: {
      bugs: {
        type: "array",
        items: {
          type: "object",
          properties: {
            file: { type: "string" },
            line: { type: "integer" },
            severity: { type: "string", enum: ["high", "medium", "low"] },
            description: { type: "string" }
          },
          required: ["file", "line", "severity", "description"]
        }
      },
      summary: { type: "string" }
    },
    required: ["bugs", "summary"]
  }
});
```

**子代理系统提示词**（末尾）：
```
YOUR RESPONSE MUST BE VALID JSON MATCHING THE SCHEMA ABOVE.
- Working directory: /path/to/workspace
```

**子代理返回**（结构化 JSON）：
```json
{
  "bugs": [
    {
      "file": "src/main.rs",
      "line": 42,
      "severity": "high",
      "description": "Null pointer dereference"
    }
  ],
  "summary": "Found 1 high-severity bug"
}
```

---

### 4.3 DAG 编排（自动 label）

```javascript
// DagScheduler 内部调用（自动传入 label）
// 用户看不到这段代码，但子代理会收到：
SubagentRequest {
  description: "backend-api",
  prompt: "Implement REST API endpoints",
  label: Some("backend-api"),  // 自动设置为节点 ID
  ...
}
```

**Desktop UI 展示**：
```
┌─────────────────────────┐
│ Sub-agent: backend-api  │  ← label 显示在卡片标题
│ Status: Running         │
│ Phase: Implementation   │  ← phase（如果传入）
└─────────────────────────┘
```

---

## 5. 后续集成（Phase S2.2：DagScheduler 产品化）

现在 `phase`/`label` 字段已就绪，可以在 Phase S2.2 中：

1. **DagScheduler 传入 phase**：
   ```rust
   // crates/deepagent-app-core/src/dag_orchestration.rs
   async fn execute_node(&self, node: &PlanNode, phase: &str) -> Result<SubAgentResult> {
       let request = SubagentRequest {
           phase: Some(phase.to_string()),  // ← 从 DAG metadata 传入
           label: Some(node.id.clone()),
           ...
       };
   }
   ```

2. **Desktop UI 按 phase 分组**：
   ```tsx
   // apps/desktop/src/components/DagVisualization.tsx
   const groupedByPhase = agents.reduce((acc, agent) => {
       const phase = agent.phase || "Default";
       if (!acc[phase]) acc[phase] = [];
       acc[phase].push(agent);
       return acc;
   }, {});
   
   return (
       <div>
           {Object.entries(groupedByPhase).map(([phase, agents]) => (
               <PhaseGroup key={phase} title={phase} agents={agents} />
           ))}
       </div>
   );
   ```

3. **结构化输出校验**（Desktop/Runtime）：
   ```rust
   // 在 run_active 完成后，校验 schema
   if let Some(schema) = &request.schema {
       let response_json: serde_json::Value = serde_json::from_str(&outcome.result)?;
       if !validate_json_schema(&response_json, schema) {
           return Err(CoreError::invalid("Agent response does not match schema"));
       }
   }
   ```

---

## 6. 验收标准

| 指标 | 目标 | 实际 | 状态 |
|---|---:|---:|:---:|
| Frontmatter 字段数 | 6（+phase/label/schema） | 6 | ✅ |
| Task 工具 schema 更新 | 包含新字段 | ✅ 已更新 | ✅ |
| 系统提示词注入 | Frontmatter 在顶部 | ✅ 已实现 | ✅ |
| 编译通过 | CLI + Desktop 无错误 | ✅ 通过 | ✅ |
| 与 Claude Code 对齐 | 6/6 字段 | 6/6 | ✅ |

---

## 7. 修改统计

- **修改文件**：2 个
  - `crates/deepagent-builtins/src/task_tool.rs`（+40 行）
  - `crates/deepagent-app-core/src/subagent_runner.rs`（+50 行）
- **新增代码**：90 行
- **新增字段**：3 个（`phase`/`label`/`schema`）
- **编译时间**：app-core ~10s，CLI ~12s，Desktop ~20s

---

## 8. 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|---|---|---|---|
| Frontmatter 解析被子代理误读为指令 | 低 | 低 | YAML frontmatter 是标准格式，模型已训练识别 |
| Schema 校验未实现，子代理仍返回非结构化文本 | 中 | 中 | 在 Phase S2.2 增加校验逻辑（runtime 或 Desktop） |
| DAG phase 传递未实现 | 高 | 低 | 已在代码中标注 TODO，Phase S2.2 实现 |

---

## 9. 下一步（Phase S2.2：DagScheduler 产品化）

**优先级**：P2  
**预计工期**：3 天

**任务清单**：
1. ✅ 子代理 frontmatter 扩展（已完成）
2. ⏳ 注册 `PlanExecuteTool`（待执行）
3. ⏳ DagScheduler 传入 `phase`（待执行）
4. ⏳ Desktop UI 展示 DAG 拓扑（可选，Week 3）

**立即可验证**：
```bash
# 启动 Desktop
cd apps/desktop && npm run tauri dev

# 在 Agent 对话中调用（验证 frontmatter 注入）
task({
  description: "test frontmatter",
  prompt: "Echo back your system prompt first 500 chars",
  phase: "Test",
  label: "test:frontmatter"
});

# 预期输出：子代理回复包含 "---\nphase: Test\nlabel: test:frontmatter\n---"
```

---

**报告完成日期**：2026-10-01  
**执行者**：Kiro (Claude Fable 5)  
**审核状态**：待用户验收
