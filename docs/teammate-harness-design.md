# Teammate（子代理）平台化 + AgentDef 扩展 + Prompt 收敛 —— 设计文档

> 状态：**设计稿（B4，design only，无代码改动）**
> 依据：`借鉴/claudecode/restored-src/src/tools/AgentTool/loadAgentsDir.ts`（Claude Code 代理定义真实 schema）、
> 本仓库 `subagent_runner.rs` / `task_tool.rs` / `deepagent-context`（`assembler`/`budget`）/
> `deepagent-prompts`（`agent_def`/`builder`）。
> 本系统是 DeepSeek 原生 Agent 运行时，一切对齐以 DeepSeek 官方能力与仓库铁律为准（AGENTS.md §5.3）。

---

## 1. 目标与范围

Teammate = 子代理（sub-agent）平台化：把现有 `task` 工具 + `ChatSubagentRunner` 的能力对齐
Claude Code 的 agent 定义体系，并把子代理生命周期投影为 harness 共享协议事件，供 CLI / SDK /
Desktop / app-server 复用 **同一套** 运行、审批、取消、持久化能力。**绝不新增第二套 run store /
event store / 审批中心 / 工具注册表**（AGENTS.md §5.1）。

本设计覆盖三个阶段的设计：

| 阶段 | 内容 | 对应计划批次 |
|---|---|---|
| Teammate-1 | `AgentDef` frontmatter 扩展（disallowedTools / permissionMode / maxTurns / effort） | B5（S2-3 前半） |
| Teammate-1b | Prompt 收敛：消除 `SystemPromptBuilder` / `PromptBudget::fit` 双实现 | B5（S2-4） |
| Teammate-2 | 子代理生命周期 → harness 协议 DTO + 事件投影 | B5 收尾 / B6 接缝 |
| Teammate-3 | TS SDK 接线（`agent/task` 请求从 SDK 直接驱动同一 runner） | 后续 |

设计原则：

1. **单一运行链**：无论入口（CLI / SDK / Desktop / app-server），子代理都由
   `SubagentRunner` 这一实现执行；`runtime` 的 `RuntimeEngine` 是唯一执行器。
2. **协议与 UI DTO 分离**：harness DTO 服务机器协议，UI DTO 仅服务视图。
3. **破坏性最小化**：扩展字段一律 `#[serde(default)]`，老 agent 文件零迁移。
4. **有据可依**：字段名与语义以 `loadAgentsDir.ts` 还原源码为准，不臆造。

---

## 2. 现状盘点（有据可依）

### 2.1 已有的子代理能力（`subagent_runner.rs` + `task_tool.rs`）

- **请求形状** `SubagentRequest`（[task_tool.rs:46](crates/deepagent-builtins/src/task_tool.rs#L46)）：
  `description / prompt / subagent_type / allowed_tools / model / effort / skills / isolation / fork`。
- **Outcome 投影** `SubagentOutcome { result, tool_use_count, duration_ms, tokens }`（CC `AgentToolResult` 对齐）。
- **运行器** `ChatSubagentRunner`：嵌套 `RuntimeEngine` 循环、sub-registry（内置减 `task` 防递归）、
  ephemeral in-memory `Session`。
- **定义收集** `collect_runtime_agent_definitions`：项目 `.deepagent/agents/` + 插件 `agent_roots` +
  内置 `explore` / `plan`；同一 `type_name` 时项目/插件覆盖内置（CC 优先级：built-in < 用户/项目）。
- **内置代理** `explore` / `plan`：只读工具白名单 `READ_ONLY_AGENT_TOOLS`（结构性只读，`apply_runtime_agent_tool_filter`）。
- **工具过滤**：正向 allowlist（`runtime_agent_tool_allowlist`）+ `normalize_runtime_agent_tool_name`
  的 CC 工具名归一化（`read_file`/`write_file`/`multi_edit`/`todo_write` 等）。
- **系统提示** `subagent_system_prompt`：base + dynamic boundary + identity + body + preloaded skills + task。
- **fork**：全 tool pool + parent response history（full-context fork，CC `tools:['*']` 对齐）。
- **隔离**：`GitWorktrees` worktree provider + `WorktreeCreate/Remove` hooks + `execution_root` 重绑定。
- **审批桥**：`parent_approvals: OnceLock<Arc<dyn ApprovalGate>>` → 子代理高风险工具冒泡到父 UI 通道，
  无 UI 时引擎默认 AutoDeny（安全兜底）。
- **压缩**：子代理带 reactive compactor + proactive compaction（按 `ContextPolicy` 阈值）。当前**无 hooks**
  （空 `HookRegistry`，hook dispatch 全部 `Continue`）。
- **生命周期事件** `RuntimeEvent::SubagentStarted / SubagentCompleted / SubagentCancelled /
  SubagentNotification`，写 run 事件（scrub 脱敏）+ sink emit。
- **后台 / 恢复 / 取消 / 状态 / 清理**：`start_background` / `resume`（D4 已核实存在，含 `resume_count`、
  transcript + `SubagentRunRecord`）/ `cancel` / `status` / `cleanup`。
- **持久化**：`SubagentRunRecord`（runtime store 侧）+ transcript JSON（attempts 数组，可重放）。

### 2.2 与 Claude Code 的差距（`loadAgentsDir.ts` 实据对比）

CC 的 agent 定义支持以下 frontmatter 字段，本仓库 `AgentDef`（[agent_def.rs:43](crates/deepagent-prompts/src/agent_def.rs#L43)）
当前只有 **name / description / tools / model / color / body**：

| CC frontmatter 字段 | CC 语义（实据） | 本仓库现状 | 处理 |
|---|---|---|---|
| `description` | → runtime `whenToUse`，给模型挑选用 | 已有 `description` | 无需改名 |
| `tools` | 允许列表 | 已有（正向 allowlist） | — |
| `disallowedTools` | **拒绝列表**（负向过滤） | ❌ 无 | **Teammate-1 新增** |
| `model` | 模型偏好（`inherit` 或命名） | 已有 `ModelPref` | — |
| `color` | UI 颜色 | 已有 | — |
| `effort` | 推理深度（`simple/medium/deep` 或整数） | 请求级 `effort` 已支持，agent 文件级 ❌ | **Teammate-1 新增** |
| `permissionMode` | 权限模式（`PERMISSION_MODES`） | ❌ 无（子代理硬编码 `PermissionSet::developer`） | **Teammate-1 新增** |
| `maxTurns` | 最大 agentic 轮数 | ❌ 无（`RuntimeConfig.max_steps=64` 全局硬编码） | **Teammate-1 新增** |
| `skills` | 预加载技能名 | 请求级 `skills` 已支持，agent 文件级 ❌ | **Teammate-1 新增** |
| `background` | 总是后台启动 | ❌ | **Teammate-1 新增** |
| `memory` | 持久记忆作用域 | ❌ | 后续 |
| `isolation` | worktree/remote | 请求级 `isolation` 已支持，agent 文件级 ❌ | **Teammate-1 新增** |
| `mcpServers` | 引用/内联 MCP server | ❌ | 后续 |
| `hooks` | agent 启动时注册 session hooks | ❌（子代理 hook dispatch 空实现） | 后续 |
| `initialPrompt` | 首个 turn 前置 | ❌ | 后续 |

> 注：此前草稿中的 `permission_base` / `when_to_use` / `max_turns` 字段名是**错误命名**。
> CC 真实字段为 `permissionMode`、`maxTurns`（camelCase），`whenToUse` 是 runtime 字段而非 frontmatter 键。
> 本设计一律以 `loadAgentsDir.ts` 为准。

### 2.3 权限现状

- 子代理硬编码 `let granted = PermissionSet::developer()`（[subagent_runner.rs:525](crates/deepagent-app-core/src/subagent_runner.rs#L525)），
  即每个子代理当前都以"开发者全权"创建工具 schema。权限差异只能靠工具 allowlist 表达。
- `ApprovalGate`（[approval.rs:62](crates/deepagent-runtime/src/approval.rs#L62)）只有 `request -> ApprovalDecision`
  单一能力；`ApprovalDecision { Allow, Deny }`。审批请求含 `tool / risk / reason / arguments`。
- `PermissionSet`（[permission.rs:60](crates/deepagent-tools/src/permission.rs#L60)）以正集方式表达授予的权限。

### 2.4 Prompt 构建链现状（S2-4 评估）

**结论：存在一套"完整但死代码"的拼接原语 + 一套活跃的拼装实现，共两套 `PromptSource`/`PromptFragment` 语义。**

| 构建器 | 位置 | 状态 |
|---|---|---|
| `ContextManifest` + `ContextAssembler::assemble(counter, token_budget)` | `deepagent-context` `assembler.rs` | ✅ 生产唯一入口（`system_context.rs` → `build_system_manifest`） |
| `PromptBudget::fit(fragments, counter)` | `deepagent-context` `budget.rs` | ⚠️ **无生产消费方**（仅 `ContextPolicy::prompt_budget` 构造器持有阈值，未接入决策）；逻辑完整、有裁剪测试，属可复用原语 |
| `SystemPromptBuilder` | `deepagent-prompts` `builder.rs` | ⚠️ **无生产消费方**（仅自身/lib tests 使用）；`deepagent-prompts/lib.rs:62` 的调用在测试内 |

- S1-2（已提交 `30bb56d`）：`assemble` 已接真实 `prompt_budget`（原 `usize::MAX`）。预算闸现在生效。
- **决策**：不接线 `PromptBudget::fit` 作为第二套裁剪运行时（避免两套 token 裁剪对同一 manifest 竞态），
  改为 **复用 `assemble` 的 `dropped_origins` 输出** 作为唯一裁决策略来源；
  `fit` 的裁剪逻辑在 `assemble` 之上做原语复用，或退役。见 §4。

---

## 3. Teammate-1：AgentDef frontmatter 扩展

### 3.1 `AgentDef` 新字段（向后兼容）

在 [agent_def.rs](crates/deepagent-prompts/src/agent_def.rs) 的 `AgentDef` 增加遗留字段
（全部 `#[serde(default)]` / `Option`，老的 `.md` 文件零迁移，解析测试保持通过）：

```rust
pub struct AgentDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub model: ModelPref,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub body: String,
    // —— Teammate-1 新增 ——
    /// `disallowedTools`（负向过滤；仍允许工具时优先生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    /// `permissionMode`：子代理运行的权限模式（映射见 §3.2）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// `maxTurns`：最大 agentic 轮数，映射 `RuntimeConfig.max_steps`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
    /// `effort`：默认推理深度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortLevel>,
    /// `skills`：默认预加载技能（请求级 `skills` 覆盖之）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// `background`：总是后台运行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// `isolation`：worktree / shared。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<IsolationChoice>,
}
```

- `PermissionMode` / `EffortLevel` / `IsolationChoice` 建议放 `deepagent-prompts`（定义层），
  与 `ModelPref` 同风格；`deepagent-tools` 已 `PermissionSet`，映射只能发生在装配侧（app-core），
  避免 prompts → tools 的反向依赖。
- `RuntimeAgentDefinition` / `subagent_system_prompt` 消费新字段展示；`task_agent_type()` 的
  description 保持模型可读。

### 3.2 `permissionMode` → `PermissionSet` 映射（专业决策点）

CC 的 `permissionMode` 是一组命名权限模式（`bypassPermissions` / `planOnly` / `acceptEdits` /
`default` / `aggressiveAccess` / `highestImpact` / `acceptedEmpowerLimit`）。

本仓库的映射以 **现有 `PermissionSet` + `ApprovalGate` 语义** 表达，**不引入第二套权限模型**（§5.5）：

| 意图（CC 语义） | 本仓库表达 |
|---|---|
| `bypassPermissions`（全放行） | `PermissionSet` 全量 + `AutoApproveGate` |
| `default` | 默认 developer 集 + 事件冒泡（现状） |
| `planOnly` / 只读 | 只读工具 allowlist（复用 `READ_ONLY_AGENT_TOOLS`）+ 默认 AutoDeny |
| 更强权限 | 扩充 `PermissionSet` + 保持审批冒泡 |

**接缝**：子代理构造时不再硬编码 `PermissionSet::developer()`，而是：

```rust
let granted = permission_set_for(agent_profile); // 依 §3.2 映射，apply_runtime_agent_tool_filter 之后
if let Some(gate) = self.parent_approvals.get() { engine = engine.with_approvals(gate.clone()); }
```

### 3.3 `maxTurns` → `RuntimeConfig.max_steps`

`run_active` 中 `RuntimeConfig { ..Default::default() }` 的 `max_steps`（默认 64）改为
`agent_profile.max_turns.unwrap_or(64)`。`StepLimitReached` 出口已存在，行为不变、无需新出口。

### 3.4 工具双面过滤：allowlist + disallowedTools

现有 `apply_runtime_agent_tool_filter` 只做正向 allowlist。新增负向过滤，**在同一函数内**

```rust
tools.retain(|tool| {
    allowlist.map_or(true, |s| s.contains(&tool.function.name))
        && !disallowed.contains(&tool.function.name)   // disallowedTools 优先
});
```

对齐 CC：`disallowedTools` 独立于 `tools`，列表用同一 `normalize_runtime_agent_tool_name` 归一化。
内置 explore/plan 用 disallowedTools 而非白名单，亦不等价重写（现有只读白名单测试保持通过）。

### 3.5 effort / skills / background / isolation 文件级默认

- `effort`：`subagent_thinking_depth(agent_profile.def.effort, inherited)` ——请求级 `effort` 覆盖。
- `skills`：`preload_skills(agent_profile.def.skills)` 与请求级 `skills` 取并集。
- `background`：`start_background` 时若 agent 文件声 `background: true`，`run()` 也走后台上报路径。
- `isolation`：请求级未显式指定时用 agent 文件默认。

### 3.6 破坏性评估（Teammate-1）

| 影响面 | 评估 |
|---|---|
| `AgentDef` 结构化 | 全新增字段 `Option/default`，现有构造点（builtin、parse、tests）**不破坏**；`serde` 反序列化老 frontmatter 兼容 |
| `RuntimeAgentDefinition` | 字段透传，无签名变化 |
| `PermissionSet::developer()` 硬编码 | `run_active` 一处替换；headless/test 无 parent gate 时仍 AutoDeny 兜底 |
| `task` 工具 schema | `TaskAgentType` description 不变；agent 文件新增字段不要求 schema 变化 |
| 内置 explore/plan | 若改为 disallowedTools 表达需同步 `READ_ONLY_AGENT_TOOLS` 测试；**保持白名单即可**，本阶段不动 |

---

## 4. Teammate-1b：Prompt 收敛（消除双实现）

### 4.1 目标状态

**唯一**裁剪运行时 = `ContextAssembler::assemble(counter, token_budget)`（生产已唯一）。

具体收敛：

1. **`SystemPromptBuilder` 退役**：`deepagent-prompts` 中 `pub use builder::SystemPromptBuilder`
   从公共 API 移除（或其模块标注 `#[doc(hidden)]` 过渡一个 release）。其分层概念
   （core/safety/workspace/identity/tool_rules/memory）已在 `system_context.rs` 的 `ContextManifest`
   落实，无二义。测试转换为对 `assemble` 的断言。
2. **`PromptBudget::fit` 原语化**：不删除，改为 `assembler::assemble` 调用的内部实现细节，
   或标注"仅被 `assemble` 使用"。**不**作为 `system_context.rs` 的第二个入口。
3. **验证**：`dropped_origins` 语义（`required` 保留 + priority-drop + 记录 drop）保持为唯一
   budget 决策来源；补一个"同一 fragment 集合在 `prompt_budget` 改变前后，`required` 恒存而
   可选块落入 `dropped_origins`"的确定性测试。

### 4.2 破坏性评估（Teammate-1b）

| 影响面 | 评估 |
|---|---|
| `deepagent-prompts` 公共 API | `SystemPromptBuilder` 移除为**破坏性公开 API 变更**；仓库内仅 tests 使用。需同步 `lib.rs` doc 提及与 `.kiro`/文档引用 |
| `deepagent-context` 公共 API | `PromptBudget::fit` 保留但改可见性/标 doc，非破坏 |
| 外部消费方 | grep 确认无 `SystemPromptBuilder` 生产调用（已核实：crates 内仅 builder.rs 自身与 lib tests） |
| 文档 | `项目指南.md`/README 若有提及需同步 |

> 决定理由：避免"两套裁剪"为同一 manifest 各算一次，导致 `required` 保留与非 required 裁剪的
> 优先级在两条代码路径上漂移；收敛到 `assemble` 一个实现后，prompt 预算变更有且仅有一个真源。

---

## 5. Teammate-2：子代理生命周期 → harness 协议投影

### 5.1 现状

已有 `RuntimeEvent` 变体（均写 run 事件 + sink）：

```text
SubagentStarted      { id, parent_run_id, agent_type, description, background }
SubagentCompleted    { id, parent_run_id, state, summary, duration_ms, background }
SubagentCancelled    { id, parent_run_id, duration_ms, background }
SubagentNotification { id, parent_run_id, state, summary }
WorktreeCreated      { subagent_id, path }
WorktreeRemoved      { subagent_id, path }
```

缺口在 **harness 协议层**：CLI JSONL / TS SDK 需要一个独立的、可重放的子代理事件 DTO，
它投影自 `RuntimeEvent`，服务机器协议，而非 UI 视图。

### 5.2 投影设计

新增 **harness DTO**（放 harness 协议 crate 或现有 job-runner 协议层，随实现方案定）：

```rust
// 机器协议事件，与 UI 的 desktop DTO 分离。
enum SubagentLifecycle {
    Started { id, parent_run_id, agent_type, description, background },
    Completed { id, state, result_ref, duration_ms, tokens, background },
    Cancelled { id, duration_ms },
    Approved  { call_id, tool, decision },   // 子代理审批冒泡，跨 line
    Resumed   { id, resume_to },             // resume 续接
}
```

接线：`ChatSubagentRunner.emit_subagent_event` 目前转向任一 `RuntimeEventSink`（desktop 通道）。
harness 集成时复用同一 sink 接口，由 harness 侧把 `RuntimeEvent` 投影为协议事件并落
**既有的** run event 存储（不新增 event store）。

### 5.3 待接缝清单（留给 Teammate-3 + SDK）

| 能力 | 状态 | 接缝 |
|---|---|---|
| `thread/start` + `task` 同线路 | 需 CLI 层 | `task` 工具经 harness 调用时子代理完整生命周期走同一 `SubagentLifecycle` |
| CLI JSONL 事件 | 需投影 | `RuntimeEvent` → JSONL 行（scrub 脱敏，复用 `scrub_secrets_value`） |
| TS SDK `agent.task` | 需 DTO | SDK 直接驱动 `SubagentRunner`（同一实现，非第二套） |
| 审批 | 已有桥 | `approval/respond` 协议通道直达 5.1 的父 gate |

---

## 6. 排序与验收

### 6.1 实现排序

1. **Teammate-1**（B5）：`AgentDef` 扩展 + 工具负向过滤 + permissionMode/maxTurns/effort 接线。
   - 单测：frontmatter 解析新字段；allowlist+disallowedTools 组合；maxTurns 映射 `max_steps`；
     permissionMode 映射到 `PermissionSet`；老 agent 文件反序列化兼容回归。
2. **Teammate-1b**（B5）：`SystemPromptBuilder` 退役 + `fit` 原语化 + 预算唯一真源断言。
   - 单测：`dropped_origins` 预算哑变断言；`assemble` 仍保留 required/full。
3. **Teammate-2**（B6 起）：harness 协议 DTO + 运行时事件投影 + CLI JSONL 事件测试串联。
   - 单测：`RuntimeEvent` → 协议事件投影往返；断线续读/replay 后子代理终态不重放。

### 6.2 验证纪律（每项必过）

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cd apps/desktop && pnpm exec tsc --noEmit
cd apps/desktop/src-tauri && cargo check
```

无头环境无法验证的行为（真实 UI 审批弹窗、多进程 CLI→SDK 联动、worktree 真机隔离效果、
DeepSeek 端到端流式）必须在提交信息中如实标注，不以"应该可以"替代。

---

## 7. 明确不做（本阶段边界）

- ❌ 第二套子代理执行器 / run store / event store / approval center / tool registry。
- ❌ `memory`（agent persistent memory scope）、`mcpServers` agent 作用域、`hooks` agent 作用域
  的完整实现 —— 列为后续；本期只做 `AgentDef` 结构级预留（可先不接受这些 frontmatter 键）。
- ❌ 将 CC 的 JSON agent 定义（`agents.json` settings）并入，除非后续证据表明本仓库需要
  settings 级 agent 源。
- ❌ 改动既有 `explore`/`plan` 只读白名单表达为 disallowedTools（测试友好且语义已等价）。
- ❌ 真机远程 daemon / HTTP transport（协议稳定后另行评估）。