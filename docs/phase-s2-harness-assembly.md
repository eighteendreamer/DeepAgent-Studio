# Phase S2：Harness 能力装配实施方案

> **优先级**：P1（产品完整性）  
> **预期工期**：3 周  
> **目标**：将已实现但未装配的底层能力接入生产主链，消除"能力孤岛"

---

## 1. 背景

当前 DeepAgent 存在 4 个"能力孤岛"（代码完整、测试通过，但未接入 CLI/Desktop 主链）：

1. **【P1】CronScheduler 未启动**：scheduler + store + tools 完整，但 `ChatService::with_cron()` 从未调用，tick loop 从未启动
2. **【P2】DagScheduler 未暴露**：真实并发 fan-out 已实现并验证（fd0709c），但 `PlanExecuteTool` 未在 UI 注册
3. **【P1】子代理 frontmatter 字段少**：仅支持 `model`/`thinking`，缺 `phase`/`label`/`schema`/`isolation` 等（Claude Code 有 10+ 字段）
4. **【P1】提示词拼接双轨**：`SystemContextAssembly` 和旧 `prompt_builder` 并存，存在不一致风险

这些导致：
- 用户无法使用定时任务功能（CronCreate 工具返回 "unavailable"）
- 多代理编排能力无法演示（尽管底层已验证）
- 子代理缺少结构化元数据（难以在 UI 追踪）

---

## 2. 实施项

### 2.1 Cron 产品化（P1）

#### 2.1.1 当前状态

**已实现**：
- ✅ `CronScheduler` (crates/deepagent-runtime/src/schedule/scheduler.rs:60) — poll 循环、持久化 store
- ✅ `CronService` (crates/deepagent-app-core/src/cron_service.rs:19) — ChatService 集成接口
- ✅ `CronCreateTool` / `CronDeleteTool` (crates/deepagent-builtins/src/cron_tools.rs:88) — 工具定义

**缺失**：
- ❌ CLI/Desktop 未调用 `ChatService::with_cron()`
- ❌ `run_tick_loop()` 从未启动
- ❌ 工具虽注册，但 `backend=None` → 运行时返回 "unavailable"

#### 2.1.2 设计

**目标架构**：
```
CLI/Desktop 启动
  → 构造 CronService::new(workspace, chat_service)
  → ChatService::with_cron(cron_service)
  → 启动后台 tick loop：tokio::spawn(cron_service.run_tick_loop())
  → 工具调用：CronCreateTool → cron_service.backend.create()
    → CronScheduler::store().add() → scheduled-tasks.json
  → Tick loop 每分钟 poll：scheduler.poll() → 匹配 cron 表达式 → 触发 ChatService::run()
```

**生命周期管理**：
- Tick loop 持续运行直到进程退出
- 用 `CancellationToken` 优雅关闭（收到 SIGTERM/SIGINT 时）

#### 2.1.3 实施路径

**步骤 1**：在 CLI 启动 Cron 服务

```rust
// apps/cli/src/main.rs:263 (after build_chat_service)
let cron_service = Arc::new(
    deepagent_app_core::CronService::new(&workspace, chat.clone())
);
let chat = chat.with_cron(cron_service.clone());

// 启动 tick loop（后台任务）
let shutdown_token = tokio_util::sync::CancellationToken::new();
let shutdown_for_cron = shutdown_token.clone();
tokio::spawn(async move {
    if let Err(e) = cron_service.run_tick_loop(shutdown_for_cron).await {
        tracing::error!(error = %e, "cron tick loop failed");
    }
});

// 注册 SIGTERM/SIGINT 处理
tokio::spawn(async move {
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("received shutdown signal");
    shutdown_token.cancel();
});
```

**步骤 2**：在 Desktop 启动 Cron 服务

```rust
// apps/desktop/src-tauri/src/lib.rs:6520 (inside start_tauri_main)
let cron_service = Arc::new(
    deepagent_app_core::CronService::new(&workspace, chat.clone())
);
let chat_service = chat_service.with_cron(cron_service.clone());

// 启动 tick loop
let shutdown_token = app_state.shutdown_token.clone();
tauri::async_runtime::spawn(async move {
    cron_service.run_tick_loop(shutdown_token).await.ok();
});
```

**步骤 3**：验证工具可用性

```rust
// crates/deepagent-app-core/src/tool_runtime.rs:528
pub fn register_cron_tools(
    registry: &mut ToolRegistry,
    cron: Option<Arc<CronService>>,
) {
    if let Some(backend) = cron {
        registry.register(Arc::new(CronCreateTool::new(backend.clone())));
        registry.register(Arc::new(CronDeleteTool::new(backend)));
    } else {
        // 未启动 CronService 时不注册工具（避免 "unavailable" 错误）
        tracing::warn!("CronService not initialized, skipping cron tools");
    }
}
```

#### 2.1.4 验证

**单元测试**（已存在）：
- ✅ `schedule/scheduler::tests::one_shot_fires_once_then_is_removed`
- ✅ `schedule/scheduler::tests::recurring_reschedules_from_fire_time`

**集成测试**（新增）：
1. CLI 启动 → 调用 `CronCreateTool("0 9 * * *", "echo hello", false)` → 验证返回 task_id
2. 等待 1 分钟 → 验证 tick loop 触发 → 检查 `ChatService::run()` 被调用
3. CLI 关闭 → 验证 tick loop 优雅退出（无 panic）

#### 2.1.5 预期收益

- 用户可用定时任务功能（对标 Claude Code 的 CronCreate/CronDelete 工具）
- 解锁自动化场景（如"每天 9 点整理日程"、"每小时检查 CI 状态"）

---

### 2.2 DagScheduler 产品化（P2）

#### 2.2.1 当前状态

**已实现**：
- ✅ `DagScheduler` (crates/deepagent-subagents/src/scheduler.rs:54) — 真实并发 fan-out（fd0709c 验证）
- ✅ `PlanDag` + `HeuristicPlanner` (crates/deepagent-planner/src/*.rs) — 3 planner 策略
- ✅ `ChatPlanExecutor` (crates/deepagent-app-core/src/dag_orchestration.rs:125) — app-core 适配器

**缺失**：
- ❌ `PlanExecuteTool` 未在 `ToolRegistry` 注册
- ❌ Desktop UI 不展示 DAG 拓扑和进度

#### 2.2.2 设计

**目标架构**：
```
用户输入："Build a full-stack app with React + FastAPI"
  → Agent 工具调用：PlanExecute(task="build app", strategy="multi_agent")
  → HeuristicPlanner::plan() → PlanDag { nodes: [architect, backend, frontend, review], edges: [...] }
  → DagScheduler::run(dag)
    → Layer 1: architect (独立)
    → Layer 2: backend, frontend, database (并发，architect 输出作为输入)
    → Layer 3: review (等待 Layer 2 全部完成)
  → 返回 ScheduleReport { results: {architect: "ok", ...}, all_succeeded: true }
```

**UI 展示**（Desktop）：
- DAG 拓扑图：Mermaid 渲染（或 Cytoscape.js）
- 节点状态：pending/running/succeeded/failed/skipped
- 实时进度：已完成 X / 总共 Y 节点

#### 2.2.3 实施路径

**步骤 1**：注册 `PlanExecuteTool`

```rust
// crates/deepagent-builtins/src/plan_execute.rs (已存在，需启用)
pub struct PlanExecuteTool {
    executor: Arc<ChatPlanExecutor>,
}

impl Tool for PlanExecuteTool {
    fn name(&self) -> &str {
        "PlanExecute"
    }
    
    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "The task to plan and execute"},
                "strategy": {
                    "type": "string",
                    "enum": ["single_agent", "multi_agent", "pipeline"],
                    "description": "Planning strategy"
                }
            },
            "required": ["task"]
        })
    }
    
    async fn execute(&self, args: ToolArgs, ctx: ToolContext) -> Result<ToolResult> {
        let task: String = args.get("task")?;
        let strategy: PlanStrategy = args.get("strategy")?.unwrap_or(PlanStrategy::MultiAgent);
        
        let dag = HeuristicPlanner.plan(&task, strategy)?;
        let report = self.executor.run_dag(dag).await?;
        
        Ok(ToolResult::success(format!(
            "Plan executed: {} nodes, {} succeeded, {} skipped",
            report.results.len(),
            report.results.values().filter(|r| r.ok).count(),
            report.skipped.len()
        )))
    }
}
```

**步骤 2**：在 `build_main_run_toolset()` 注册工具

```rust
// crates/deepagent-app-core/src/tool_runtime.rs:450
pub fn build_main_run_toolset(...) -> ToolRegistry {
    let mut registry = build_base_tool_registry();
    
    // ...（其他工具）
    
    // 注册 PlanExecute
    if let Some(plan_executor) = plan_executor {
        registry.register(Arc::new(PlanExecuteTool::new(plan_executor)));
    }
    
    registry
}
```

**步骤 3**：在 `ChatService` 传入 `ChatPlanExecutor`

```rust
// crates/deepagent-app-core/src/chat_service.rs:250
pub struct ChatService {
    // ...
    plan_executor: Option<Arc<ChatPlanExecutor>>, // 新增
}

impl ChatService {
    pub fn with_plan_executor(mut self, executor: Arc<ChatPlanExecutor>) -> Self {
        self.plan_executor = Some(executor);
        self
    }
}
```

**步骤 4**：Desktop UI 展示 DAG（可选，Week 3）

```tsx
// apps/desktop/src/components/DagVisualization.tsx
export function DagVisualization({ dag, report }: Props) {
  const mermaidCode = `
    graph TD
      ${dag.nodes.map(n => `${n.id}[${n.task}]`).join('\n')}
      ${dag.edges.map(e => `${e.from} --> ${e.to}`).join('\n')}
      
      classDef succeeded fill:#10b981
      classDef failed fill:#ef4444
      classDef running fill:#3b82f6
      
      ${Object.entries(report.results).map(([id, r]) => 
        `class ${id} ${r.ok ? 'succeeded' : 'failed'}`
      ).join('\n')}
  `;
  
  return <Mermaid chart={mermaidCode} />;
}
```

#### 2.2.4 验证

**集成测试**：
1. 调用 `PlanExecuteTool("build app", "multi_agent")` → 验证返回 `ScheduleReport`
2. 检查 `DagScheduler::run()` 日志 → 验证 Layer 2 节点并发执行（时间戳重叠）
3. 人工检查：architect 完成后 backend 才开始（依赖顺序正确）

#### 2.2.5 预期收益

- 解锁多代理编排演示场景
- 对外展示"DeepAgent 不只是单 agent，支持复杂工作流"

---

### 2.3 子代理 Frontmatter 扩展（P1）

#### 2.3.1 当前状态

**已支持字段**（参考 `subagent_runner.rs:420`）：
```rust
SubAgentOptions {
    model: Option<String>,       // ✅ 已实现
    thinking_mode: Option<String>, // ✅ 已实现
    // 以下缺失：
    phase: Option<String>,       // ❌ DAG 节点所属阶段（如 "Review"）
    label: Option<String>,       // ❌ UI 展示标签（如 "review:correctness"）
    schema: Option<JsonSchema>,  // ❌ 结构化输出 schema
    isolation: Option<String>,   // ❌ 隔离模式（worktree/session/none）
}
```

#### 2.3.2 设计

**目标**：对齐 Claude Code 的 agent frontmatter（`借鉴/claudecode/restored-src/src/agent.ts:180`）：
- `phase`：DAG 阶段标签（在 UI 分组展示）
- `label`：短标签（如 "review:bugs"）
- `schema`：结构化输出（JSON Schema，agent 必须返回符合 schema 的 JSON）
- `isolation`：隔离级别（worktree=独立目录、session=独立历史、none=共享）

#### 2.3.3 实施路径

**步骤 1**：扩展 `SubAgentOptions`

```rust
// crates/deepagent-app-core/src/subagent_runner.rs:80
#[derive(Debug, Clone, Default)]
pub struct SubAgentOptions {
    pub model: Option<String>,
    pub thinking_mode: Option<String>,
    pub phase: Option<String>,        // 新增
    pub label: Option<String>,        // 新增
    pub schema: Option<serde_json::Value>, // 新增
    pub isolation: Option<String>,    // 新增（"worktree" / "session" / "none"）
}
```

**步骤 2**：在系统提示词注入 frontmatter

```rust
// crates/deepagent-app-core/src/subagent_runner.rs:420
fn build_subagent_system_prompt(opts: &SubAgentOptions) -> String {
    let mut frontmatter = Vec::new();
    
    if let Some(model) = &opts.model {
        frontmatter.push(format!("model: {}", model));
    }
    if let Some(thinking) = &opts.thinking_mode {
        frontmatter.push(format!("thinking: {}", thinking));
    }
    if let Some(phase) = &opts.phase {
        frontmatter.push(format!("phase: {}", phase));
    }
    if let Some(label) = &opts.label {
        frontmatter.push(format!("label: {}", label));
    }
    if let Some(schema) = &opts.schema {
        frontmatter.push(format!("schema: {}", serde_json::to_string(schema).unwrap()));
    }
    
    format!(
        "---\n{}\n---\n\nYou are a subagent. {}",
        frontmatter.join("\n"),
        if opts.schema.is_some() {
            "Your response MUST be valid JSON matching the schema above."
        } else {
            ""
        }
    )
}
```

**步骤 3**：在 `DagScheduler` 传入 phase

```rust
// crates/deepagent-app-core/src/dag_orchestration.rs:160
async fn execute_node(&self, node: &PlanNode, phase: &str) -> Result<SubAgentResult> {
    let opts = SubAgentOptions {
        phase: Some(phase.to_string()),
        label: Some(node.id.clone()),
        isolation: Some("worktree".to_string()),
        ..Default::default()
    };
    
    self.subagent_runner.spawn_agent(&node.task, opts).await
}
```

**步骤 4**：在 Desktop UI 展示 phase/label

```tsx
// apps/desktop/src/components/SubAgentCard.tsx
export function SubAgentCard({ agent }: Props) {
  return (
    <div className="agent-card">
      <Badge variant="outline">{agent.phase}</Badge>
      <span className="label">{agent.label}</span>
      <p>{agent.status}</p>
    </div>
  );
}
```

#### 2.3.4 验证

**单元测试**：
- `subagent_runner::tests::test_frontmatter_injection()` — 验证所有字段正确注入

**集成测试**：
- 启动子代理，传入 `phase: "Review"`, `label: "review:bugs"` → 检查系统提示词包含这些字段

#### 2.3.5 预期收益

- Desktop UI 可按 phase 分组展示子代理（如"Review 阶段：3 个子代理运行中"）
- 结构化输出（schema）支持 agent → agent 数据传递（如 review agent 返回 `{bugs: [...], score: 8.5}`）

---

### 2.4 提示词拼接单源收敛（P1）

#### 2.4.1 当前状态

**双轨并存**：
- 新路径：`SystemContextAssembly` (crates/deepagent-context/src/system_context.rs) — 结构化 sections
- 旧路径：`prompt_builder` (crates/deepagent-app-core/src/prompt_builder.rs) — 字符串拼接

**风险**：
- 两套逻辑可能不一致（如一个加了 `[MCP_TOOLS]`，另一个忘了）
- 维护成本高（改一个提示词，需改两处）

#### 2.4.2 设计

**目标**：
1. 全面切换到 `SystemContextAssembly`
2. 删除 `prompt_builder.rs`（或标记 `#[deprecated]`）

#### 2.4.3 实施路径

**步骤 1**：Grep 所有 `prompt_builder` 调用点

```bash
cd /g/Code_Warehouse/DeepAgent-Studio
grep -rn "prompt_builder::" crates/deepagent-app-core/src/*.rs
```

**步骤 2**：逐个迁移到 `SystemContextAssembly`

```rust
// Before (旧代码)
use crate::prompt_builder;
let prompt = prompt_builder::build_prompt(&session, &context);

// After (新代码)
use deepagent_context::SystemContextAssembly;
let assembly = SystemContextAssembly::new()
    .with_identity(context.identity)
    .with_rules(context.rules)
    .with_skills(context.skills);
let prompt = assembly.build_sections().render();
```

**步骤 3**：删除 `prompt_builder.rs`

```bash
git rm crates/deepagent-app-core/src/prompt_builder.rs
```

**步骤 4**：验证编译通过

```bash
cargo check --workspace --all-targets
cargo test --workspace
```

#### 2.4.4 验证

**集成测试**：
- 迁移前后，对比同一 session 的 prompt 字符串（应完全一致）

#### 2.4.5 预期收益

- 单一真源（Single Source of Truth）
- 降低维护成本

---

## 3. 实施顺序

按依赖关系和优先级：

1. **Week 1, Day 1-2**：2.1 Cron 产品化（P1，高 ROI）
2. **Week 1, Day 3-5**：2.3 子代理 Frontmatter 扩展（P1，中 ROI）
3. **Week 2, Day 1-3**：2.2 DagScheduler 产品化（P2，需 2.3 完成才有 phase 传递）
4. **Week 2, Day 4-5**：2.4 提示词拼接单源收敛（P1，技术债）
5. **Week 3, Day 1-5**：集成测试 + Desktop UI 适配 + 文档更新

---

## 4. 验收标准

| 指标 | 当前 | 目标 |
|---|---:|---:|
| CronCreateTool 可用性 | unavailable | 可调用并返回 task_id |
| Tick loop 运行时长 | 0s（从未启动） | 持续运行至进程退出 |
| PlanExecuteTool 注册 | 未注册 | 已注册且可调用 |
| 子代理 frontmatter 字段数 | 2（model/thinking） | 6（+phase/label/schema/isolation） |
| prompt_builder.rs 调用数 | ~5 处 | 0（已删除） |

---

## 5. 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|---|---|---|---|
| Tick loop 后台任务泄漏（未关闭） | 中 | 中 | 用 CancellationToken 确保优雅退出 |
| DagScheduler 并发竞争条件 | 低 | 高 | 已有单元测试覆盖（fd0709c） |
| 提示词迁移遗漏某处调用 | 中 | 高 | 全局 grep + 编译检查 |
| Desktop UI 适配工作量超预期 | 高 | 低 | UI 展示为可选项（Week 3），核心功能不依赖 |

---

**报告完成日期**：2026-10-01  
**所需资源**：1 名开发者全职 3 周  
**前置条件**：Phase S1 完成（上下文预算稳定后再装配新能力）  
**后续 Phase**：S3（产品体验优化）
