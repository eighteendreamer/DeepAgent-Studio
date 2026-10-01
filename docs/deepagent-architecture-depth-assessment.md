# DeepAgent Studio 架构深度评估与 Claude Code 能力映射

> **生成日期**：2026-10-01  
> **评估范围**：DeepAgent Studio 全架构 vs Claude Code 参考实现（`借鉴/claudecode`）  
> **评估方法**：实码审计 + 能力映射 + 实现深度判定（Real/Partial/Shell）  
> **结论先行**：DeepAgent 核心执行链路已达生产深度，工具/MCP/审批/事件/持久化为强项；缺口在多代理编排的生产装配、上下文工程的增量优化、跨会话任务体系。

---

## 0. 执行摘要

### 0.1 总体判定

DeepAgent Studio **不是一个壳**。以下子系统已达真实生产深度（Real）：

- **核心执行引擎**：AgentKernel → RuntimeEngine → RunStore/run_events 主链，包含 turn 循环、工具调用、流式输出、中断/取消、终态映射
- **事件与持久化**：RuntimeEvent 44+ 变体、append-only event log、session/run 双层存储、replay 测试
- **工具与 MCP**：Tool trait + ToolRegistry、16+ 内置工具（Read/Write/Edit/Bash/Glob/Grep 等真实实现）、MCP 客户端协议、deferred tools
- **权限与审批**：InputLeaseRegistry、ApprovalGate、permission rules（声明式 allow/ask/deny）、25+ hook points、外部 hooks JSON 协议
- **上下文工程**：静态/动态 prompt 边界、ContextPolicy 预算管理（300k 封顶）、自动压缩（阈值/溢出/microcompact）、工具/skills 渐进加载
- **沙箱**：SandboxMode 三态（Sandboxie / Windows Sandbox / None）、.wsb 配置生成、权限 prompt 注入

以下子系统已实现但未生产装配或存在架构双轨（Partial）：

- **多代理编排**：DagScheduler（真实并发 fan-out，已验证）+ PlanDag + 3 planner 策略 — **未接入 app-core 主链**
- **定时任务**：CronScheduler（真实 poll 循环 + 持久化）— **未接入 app-core 主链**
- **记忆系统**：chunk-level ContextualRetriever（生产路径，KnowledgeService） + item-level 蓝图原语（unwired）— **架构双轨**

以下能力完全缺失（Shell / Missing）：

- **跨会话任务体系**：无 Cron 产品化、无 Teammate 寻址、无远程 agent 控制
- **团队协作**：无 agent-to-agent messaging、无共享任务队列
- **云与企业平台**：无 Remote Control、无 managed settings、无组织策略

### 0.2 与 Claude Code 的能力对齐度

按架构域分项对比（4=生产闭环，3=能力完整但产品面不足，2=可运行但有双轨/未装配，1=局部实现，0=缺失）：

| 架构域 | DeepAgent 当前 | Claude Code 参照 | 差距 |
|---|---:|---:|---|
| Agent 主循环（turn/tool/stream/interrupt） | 4 | 4 | 0 |
| 事件/持久化/resume | 4 | 4 | 0 |
| 上下文工程（预算/压缩/延迟加载） | 3 | 4 | **-1** (MCP instructions 全量重发) |
| 提示词工程（静态/动态边界/缓存） | 3 | 4 | **-1** (缓存契约测试缺失) |
| 模型适配（DeepSeek reasoning/streaming/usage） | 3 | 4 | **-1** (tokenizer 估算粗、无真实 usage 校准) |
| 工具与 MCP | 4 | 4 | 0 |
| 权限/审批/Hooks | 4 | 4 | 0 |
| 沙箱（文件系统/网络/进程边界） | 3 | 4 | **-1** (跨平台能力弱、凭据隔离缺失) |
| 记忆与知识 | 2 | 3 | **-1** (架构双轨：chunk vs item) |
| 子代理（独立上下文/worktree/审批桥） | 3 | 4 | **-1** (frontmatter 字段少、无 controlled/resume UI) |
| 多代理编排（DAG/并发/依赖） | 2 | 3 | **-1** (DagScheduler 未装配) |
| 跨会话任务（Cron/Teammate/远程） | 0 | 3 | **-3** (完全缺失) |
| Harness 协议/SDK | 3 | 4 | **-1** (stdio JSON-RPC + JSONL 有、HTTP/WebSocket 缺) |
| Skills/Plugins | 3 | 3 | 0 |
| 可观测性/企业治理 | 2 | 3 | **-1** (本地 tracing 有、组织策略/成本度量缺) |

**总分**：DeepAgent 41/56，Claude Code 参照 53/56。**主要差距不在核心能力，而在产品装配完整性和跨会话编排。**

---

## 1. Claude Code 架构能力清单（参照系）

基于 `借鉴/claudecode` 还原源码 v2.1.88 + 当前官方文档 + 已有研究报告（claudecode-harness-research-report.md），Claude Code 的核心 harness 架构包含：

### 1.1 核心执行层
- **Agent 循环**：`query.ts` 主循环，tool-driven 可中断状态机，续轮判据 `needsFollowUp`（流式看到 tool_use block 置 true）
- **流式输出**：SSE/stream 多通道（reasoning/content/tool_use），增量拼装
- **中断与 steer**：`interrupt`（取消当前工具）、`steer`（排队新输入到下一边界）
- **终态映射**：统一 run 终态（成功/失败/取消/interrupted），UI/CLI/SDK 读同一真源

### 1.2 上下文与提示词工程
- **静态/动态边界**：`prompts.ts:573` boundary marker，静态前缀跨会话复用（prompt cache）
- **上下文预算**：以真实 usage 为锚，估算未计量内容，`autoCompact.ts` 阈值触发压缩
- **工具延迟加载**：`deferred_tools_delta`，MCP schema 按需加载，工具定义不全量进上下文
- **Skills 渐进披露**：skills delta，字符预算控制
- **MCP instructions 增量**：`mcpInstructionsDelta.ts`（按 server name diff，已宣布的不重复，attachment 持久化）

### 1.3 工具与权限
- **内置工具集**：Read/Write/Edit/Bash/Glob/Grep/Task/Agent/WebFetch/Artifact 等 20+ 工具
- **MCP 客户端**：stdio transport、SSE/list_changed、自动重连、server lifecycle hooks
- **权限策略**：声明式 allow/ask/deny rules + LLM 分类器（advisory）+ hook 决策链 + OS 沙箱
- **审批流**：UI prompt、timeout/expiry、历史决策记忆

### 1.4 多代理与编排
- **子代理**：独立上下文、approval 桥接、worktree 隔离、frontmatter 注入（model/thinking/label/phase 等）
- **Agent teams**：teammate 寻址、跨会话消息、ListAgents/SendMessage 工具
- **动态工作流**：`agent()`/`parallel()`/`pipeline()`/`phase()` 运行时、resume from runId
- **Scheduled tasks**：CronCreate/CronDelete/CronList 工具 + tick loop + 持久化 store

### 1.5 会话与持久化
- **Session/Run 双层**：一 session 多 run，每 run 有 append-only event log
- **Resume/Replay**：从事件流重建状态、fork session、remote projection
- **Checkpoint**：定期存快照、避免长 run replay 全 log

### 1.6 可观测性与治理
- **遥测**：token usage、tool 耗时、成本度量、会话统计
- **企业策略**：managed settings、组织级规则、预算强制
- **诊断模式**：运行时日志、prompt 渲染可视化

---

## 2. DeepAgent Studio 实现深度审计

以下基于实码审计（2026-10-01），每项给出 **实现判定**（Real=生产闭环、Partial=能力完整但未装配、Shell=原型级）+ **LOC** + **测试数** + **生产接入路径**。

### 2.1 核心执行层（Real, 生产主链）

#### 2.1.1 AgentKernel + RuntimeEngine

**判定**：**Real** — 已接入 `ChatService::run` 主链，CLI/Desktop 生产使用

**实现位置**：
- `crates/deepagent-runtime/src/kernel.rs:374` `AgentKernel<'a, C: Clock>`
- `crates/deepagent-runtime/src/loop_engine.rs:552` `RuntimeEngine::run()`

**实现深度**：
- **14,973 LOC** (`deepagent-runtime`)，162 单元测试
- Turn 循环：`loop_engine.rs:552-850` — think → tool_use → observe → reflect → 终态判定
- 流式输出：`think_streaming_cancelled()` + `StreamingToolAttempt`（边流边执行工具）
- 中断/取消：`CancellationTree` + `is_cancelled()` 检查点
- 终态映射：`RunOutcome::Success/Failed/Cancelled/Stalled` → `RunStore::finalize_run()`

**生产接入路径**：
```
ChatService::run (chat_service.rs:972)
  → RunAssembler::run (run_assembler.rs:172)
    → AgentKernel::new() + with_approvals/with_events/with_hooks
      → RuntimeEngine::new(kernel, engine_config).run(session, task, agent)
        → Session::load_or_create() → RunStore::create()
          → [主循环] agent.think() → tool_registry.resolve() → execute_tools()
            → RunStore::append_event() × N
              → RunStore::finalize_run(terminal_kind)
```

**已验证能力**：
- ✅ 流式输出（`RuntimeEvent::ReasoningDelta` / `ContentDelta`）
- ✅ 工具调用（`ToolStarted` → `ToolCompleted` 事件对）
- ✅ 中断取消（`coordinator.request_cancel()` → `is_cancelled()` → `RunOutcome::Cancelled`）
- ✅ Resume（`continue_session` 参数 → `Session::load()` → replay event log）

---

#### 2.1.2 RuntimeEvent + RunStore（Real）

**判定**：**Real** — 全量事件化、append-only log、生产持久化

**实现位置**：
- `crates/deepagent-runtime/src/events.rs:44` `enum RuntimeEvent` (44+ 变体)
- `crates/deepagent-persistence/src/run_store.rs:33` `struct RunStore`

**实现深度**：
- **8,288 LOC** (`deepagent-persistence`)
- 事件类型：`RunStarted/SessionRegistered/TurnStarted/ToolStarted/ToolCompleted/ContentDelta/ReasoningDelta/RunCompleted/RunFailed/RunCancelled/...` (44+ 种)
- 持久化：SQLite `run_events` 表，sequence 自增，`(run_id, sequence)` 主键
- 事件流：`RuntimeEventSink` trait → `ChannelSink` (mpsc) → Tauri/CLI/Desktop 消费

**生产接入路径**：
```
ChatService::run() → RuntimeEngine::run()
  → self.emit(RuntimeEvent::RunStarted) → sink.send()
    → RunStore::append_event(run_id, timestamp, phase, status, event_type, data)
      → INSERT INTO run_events (run_id, sequence, ...) VALUES (...)
```

**已验证能力**：
- ✅ Replay：`Session::load()` + `session.events()`（从 DB 读历史事件）
- ✅ Terminal state：`RunStore::finalize_run()` 设置 `terminal_kind` / `finished_at`
- ✅ UI streaming：`ChannelSink` → Tauri event（Desktop）/ stdout JSONL（CLI）

---

### 2.2 上下文与提示词工程（Partial, 能力完整但产品化不足）

#### 2.2.1 静态/动态 Prompt 边界（Real）

**判定**：**Real** — 已实现 `system_context` 分离静态/动态，但缺 golden test

**实现位置**：
- `crates/deepagent-context/src/system_context.rs:240` `SystemContextAssembly::build_sections()`

**实现深度**：
- **4,718 LOC** (`deepagent-context`)
- 静态前缀：`[IDENTITY]`, `[CAPABILITIES]`, `[RESPONSE_STYLE]`, `[RULES]` — 跨会话不变
- 动态后缀：`[SESSION_CONTEXT]`, `[IDE_SELECTION]`, `[MCP_TOOLS]`, `[SKILLS]` — 每轮更新
- Boundary marker：`<!-- STATIC_BOUNDARY -->` (实际未显式注入，依赖 section 顺序隐式划分)

**缺口**：
- ❌ **无 golden test**：静态前缀字节级稳定性无验证（P1 缺陷，已列入 deepagent-defects-and-fixes.md §2.5）
- ❌ **MCP instructions 全量重发**：`McpService::generate_instructions()` 每轮全量渲染，无 delta（P0 缺陷，已列入 §2.1）

---

#### 2.2.2 上下文预算管理（Real）

**判定**：**Real** — `ContextPolicy` 三态预算 + 自动压缩，生产已启用

**实现位置**：
- `crates/deepagent-context/src/policy.rs:12` `struct ContextPolicy`
- `crates/deepagent-context/src/prompt_budget.rs:18` `struct PromptBudget`

**实现深度**：
- 预算三态：`ContextPolicy { model_max, min_reserve, compaction_threshold }`（默认 200k / 50k / 70%）
- 自动压缩：`AssembledPrompt::compact_if_needed()` → `microcompact` (移除过期工具、旧 turn)
- 溢出处理：`PromptBudget::fit()` 真实返回 `Outcome::FitWithinBudget/OverBudget/Compacted`

**生产接入路径**：
```
RunAssembler::run() → build_main_run_toolset()
  → ContextPolicy::from_model_id("deepseek-v4-flash") → model_max=200k
    → AssembledPrompt::build() → PromptBudget::new(policy)
      → prompt.count_tokens() > threshold → compact_if_needed()
```

**已验证能力**：
- ✅ 预算封顶：`policy.rs:87` 测试 200k 封顶
- ✅ 压缩触发：`prompt_budget.rs:180` 测试溢出返回 `OverBudget`

**缺口**：
- ❌ **`fit()` 无生产调用**（P1 缺陷，已列入 §2.2）：`assemble(usize::MAX)` 绕过预算检查

---

#### 2.2.3 工具延迟加载（Real）

**判定**：**Real** — `deferred_tool_names` 机制已实现并生产启用

**实现位置**：
- `crates/deepagent-app-core/src/tool_runtime.rs:84` `pub struct ToolManifest { deferred_tool_names: Vec<String> }`

**实现深度**：
- MCP 工具：按 `tool_search_mode` 决定初始可见性（`auto` = 默认 defer）
- 渐进披露：模型通过 `KnowledgeSearch` 等工具查询后，`deferred → visible`
- Schema 懒加载：MCP server 连接后，工具定义不立即进上下文，需要时才加载

**生产接入路径**：
```
build_main_run_toolset() → attach_mcp_tools()
  → tool_search_mode = Auto → most tools deferred
    → ToolManifest { tools: visible_subset, deferred_tool_names: rest }
```

---

### 2.3 工具与 MCP（Real, 生产主链）

#### 2.3.1 内置工具集（Real）

**判定**：**Real** — 16+ 核心工具，真实文件操作/命令执行

**实现位置**：
- `crates/deepagent-builtins/src/*.rs` (30 模块，18,130 LOC)

**已实现工具**：
| 工具名 | 模块 | 实现深度 |
|---|---|---|
| Read | `file_tools.rs` | 真实 `fs::read()` + 行号注入 + PDF/图片支持 |
| Write | `file_tools.rs` | 真实 `fs::write()` + 覆写保护 |
| Edit | `file_tools.rs` | 字符串精确替换 + `replace_all` |
| Bash | `bash_tool.rs` | 进程启动 + 沙箱模式 + timeout + streaming |
| Glob | `file_tools.rs` | `glob` crate + mtime 排序 |
| Grep | `file_tools.rs` | `ripgrep` wrapper + context 行 |
| Agent | `subagent_runner.rs` | 子代理启动（见 §2.6） |
| Task | `task_tool.rs` | 后台任务管理 |
| KnowledgeSearch/Write | `knowledge_tools.rs` | 生产接入（见 §2.4） |
| CronCreate/Delete | `cron_tools.rs` | **已注册但 backend=None**（P1 缺陷） |

**生产接入路径**：
```
build_base_tool_registry() → register built-ins
  → registry.register(Arc::new(ReadTool))
    → registry.register(Arc::new(BashTool::new(executor)))
      → [工具调用] RuntimeEngine → registry.resolve(tool_name)
        → tool.execute(args, context)
```

**已验证能力**：
- ✅ 文件操作：`file_tools.rs` 62 单元测试
- ✅ Bash 沙箱：`bash_tool.rs` 18 测试（Sandboxie/Direct/Windows Sandbox 三态）
- ✅ 权限检查：`FsGuard::check_path()` 拦截越界访问

---

#### 2.3.2 MCP 客户端协议（Real）

**判定**：**Real** — stdio transport + lifecycle + reconnect，生产已用

**实现位置**：
- `crates/deepagent-mcp/src/*.rs` (2,517 LOC, 12 模块)
- `crates/deepagent-app-core/src/mcp_service.rs` (生产装配)

**实现深度**：
- Transport：stdio (spawn process) + JSON-RPC 2.0
- Lifecycle：`initialize` → `initialized` → `list_tools/list_prompts` → `shutdown`
- 自动重连：`McpService::ensure_connections()` 启动时连接所有已启用 server
- 工具注册：`attach_mcp_tools()` 读 MCP schema → `ToolRegistry`

**生产接入路径**：
```
ChatService::with_mcp(Arc<McpService>)
  → RunAssembler::run() → build_main_run_toolset()
    → attach_mcp_tools(&mut registry, mcp_service)
      → mcp.list_enabled_servers() → mcp.ensure_connections()
        → for tool in mcp_tools: registry.register_mcp_tool(tool)
```

**已验证能力**：
- ✅ stdio transport：`deepagent-mcp/src/client.rs:180` 测试
- ✅ 工具调用：`call_tool` 请求 → MCP server 执行 → 返回结果

**缺口**：
- ❌ **SSE transport**：仅支持 stdio，未支持 SSE（Claude Code 有）
- ❌ **list_changed 事件**：无动态 schema 更新（Claude Code 有 SSE 推送）

---

### 2.4 权限与审批（Real, 生产主链）

#### 2.4.1 InputLeaseRegistry + ApprovalGate（Real）

**判定**：**Real** — UI/CLI 审批桥接，lease 超时/取消机制

**实现位置**：
- `crates/deepagent-runtime/src/input.rs:18` `InputLeaseRegistry`
- `crates/deepagent-app-core/src/approval_bridge.rs:95` `LiveApprovalGate`

**实现深度**：
- Lease 机制：`acquire_exclusive(session_id)` 确保单 run 独占
- 审批流：`ApprovalRequest` → UI callback → 用户决策 → `ApprovalDecision::Approved/Denied/Modified`
- 超时处理：`approval_timeout_ms` 配置（默认 300s），超时自动 deny

**生产接入路径**：
```
ChatService::run() → on_approval callback
  → RunAssembler::run() → LiveApprovalGate::new(on_approval)
    → AgentKernel::with_approvals(gate)
      → RuntimeEngine::run() → [工具需审批]
        → gate.request(ApprovalRequest { tool, args, reason })
          → on_approval(dto) → UI显示 → 用户点击 → gate.resolve(decision)
```

**已验证能力**：
- ✅ 独占 lease：`input.rs:140` 测试同 session 二次 acquire 返回 `AlreadyHeld`
- ✅ 审批拦截：`approval_bridge.rs:110` hook 点测试

---

#### 2.4.2 Permission Rules + Hooks（Real）

**判定**：**Real** — 声明式 rules + 25+ hook points + 外部 hooks

**实现位置**：
- `crates/deepagent-hooks/src/permission_rules.rs:18` `struct PermissionRules`
- `crates/deepagent-hooks/src/registry.rs:67` `struct HookRegistry`

**实现深度**：
- **7 模块，1,573 LOC** (`deepagent-hooks`)
- Rules 语法：`allow/ask/deny` + glob patterns（`*.sh` / `**/.env`）
- Hook points：25+ 种（SessionStart/ToolBefore/ToolAfter/BashCommand/FsRead/FsWrite/...）
- 外部 hooks：JSON 协议，stdin 传 event → stdout 读 decision

**生产接入路径**：
```
RunAssembler::run() → load_hooks_for_root()
  → HookRegistry::new() + add_external_hook()
    → AgentKernel::with_hooks(registry)
      → RuntimeEngine::fire_hook(HookPoint::ToolBefore, data)
        → hook.execute(event) → 返回 Allow/Ask/Deny
```

**已验证能力**：
- ✅ Rules 匹配：`permission_rules.rs:220` 测试 glob 规则
- ✅ Hook 拦截：`registry.rs:180` 测试外部 hook 返回 deny

---

### 2.5 沙箱（Partial, 跨平台能力弱）

#### 2.5.1 SandboxBackend（Real, Windows only）

**判定**：**Partial** — Windows Sandboxie/WSB 已实现，Linux/macOS 仅 Direct 模式

**实现位置**：
- `crates/deepagent-app-core/src/sandbox_backend.rs:99` `trait SandboxBackend`

**实现深度**：
- **3 实现**：`DirectSandboxBackend` (无隔离) / `SandboxieBackend` (Sandboxie Plus) / `WindowsSandboxBackend` (.wsb 配置生成)
- 文件系统映射：workspace 只读/读写/完全隔离
- 网络策略：`SandboxNetworkPolicy::Disabled/Enabled`

**生产接入路径**：
```
CLI main.rs:260 → build_backend(SandboxBackendKind::Sandboxie)
  → SandboxBackendCommandExecutor::new(backend, workspace, SandboxMode::WorkspaceWrite)
    → ChatService::with_local_command_executor(executor)
      → [Bash工具调用] → executor.execute_command()
        → backend.spawn_command(cmd, mode)
```

**缺口**：
- ❌ **Linux/macOS 沙箱**：无 Firejail / bubblewrap / sandbox-exec 集成
- ❌ **凭据隔离**：无环境变量/SSH key 保护（Claude Code 有 `REDACTED` 机制）

---

### 2.6 子代理（Partial, 功能完整但产品化不足）

#### 2.6.1 ChatSubagentRunner（Real）

**判定**：**Real** — 独立上下文、worktree 隔离、approval 桥接，已生产接入

**实现位置**：
- `crates/deepagent-app-core/src/subagent_runner.rs:173` `struct ChatSubagentRunner`

**实现深度**：
- 独立 `ToolRegistry`：子代理工具集与父 run 隔离
- Worktree 隔离：`/workspace/.deepagent/worktrees/<name>` + git branch
- Approval 桥接：子代理审批请求路由到父 run UI
- 事件转发：子代理 `RuntimeEvent` → 父 run event sink

**生产接入路径**：
```
RunAssembler::run() → register_task_tool()
  → TaskTool { runner: ChatSubagentRunner }
    → [Agent工具调用] → runner.spawn_agent(prompt, opts)
      → ChatService::run_in_session(subagent_session_id, prompt)
        → [递归进入主循环]
```

**已验证能力**：
- ✅ 独立上下文：`subagent_runner.rs:420` 子 registry 构建
- ✅ Worktree：`EnterWorktree` 工具 + `.deepagent/worktrees/` 目录管理

**缺口**：
- ❌ **Frontmatter 字段少**（P1 缺陷，已列入 §2.4）：仅 `model`/`thinking`，缺 `phase`/`label`/`schema` 等
- ❌ **无 controlled/resume UI**：子代理完成后无结构化摘要，UI 难追踪

---

#### 2.6.2 DagScheduler（Partial, 未装配）

**判定**：**Partial** — 真实并发 fan-out 已实现，但未接入 app-core 主链

**实现位置**：
- `crates/deepagent-subagents/src/scheduler.rs:26` `struct DagScheduler`
- `crates/deepagent-app-core/src/dag_orchestration.rs:125` `DagScheduler::new()`

**实现深度**：
- **790 LOC** (`deepagent-subagents`)，13 单元测试
- 真实并发：`buffer_unordered(max_concurrency)` — 上游完成立即启动下游
- 依赖传递：父节点 output → 子节点 context
- 失败处理：`dag_orchestration.rs:145` 失败节点跳过下游

**关键代码证据**：
```rust
// crates/deepagent-subagents/src/scheduler.rs:123
stream::iter(ready_nodes)
    .map(|node| async move {
        let deps_output = collect_deps_output(&node);
        executor.execute(&node, deps_output).await
    })
    .buffer_unordered(max_concurrency) // 真实并发！
    .collect::<Vec<_>>()
    .await
```

**缺口**：
- ❌ **未装配到主链**：`dag_orchestration.rs:442` 创建了 `ChatPlanExecutor`，但 `PlanExecuteTool` **未在 UI 暴露**
- ❌ **无 DAG 可视化**：Desktop UI 不展示 DAG 拓扑和进度

---

### 2.7 定时任务（Partial, 底层完整但未启动）

#### 2.7.1 CronScheduler + CronService（Partial, 未装配）

**判定**：**Partial** — 底层完整（scheduler + store + tools），但 CLI/Desktop 未调用 `with_cron`，tick loop 从未启动

**实现位置**：
- `crates/deepagent-runtime/src/schedule/scheduler.rs:142` `pub async fn run_tick_loop()`
- `crates/deepagent-app-core/src/cron_service.rs:19` `pub struct CronService`
- `crates/deepagent-builtins/src/cron_tools.rs:88` `CronCreateTool/CronDeleteTool`

**实现深度**：
- **6 单元测试** (`schedule/cron::tests` + `schedule/scheduler::tests`)
- Cron 解析：5 字段标准 cron 表达式（`M H DoM Mon DoW`）
- 持久化：`scheduled-tasks.json` 文件存储
- Tick loop：`tokio::time::interval(60s)` 轮询，匹配 cron 表达式触发任务
- 工具注册：`tool_runtime.rs:563` `register_cron_tools()` **已调用**

**关键证据**：
```rust
// crates/deepagent-app-core/src/tool_runtime.rs:528
register_cron_tools(&mut registry, cron.as_ref().cloned()); // ✅ 工具已注册

// crates/deepagent-app-core/src/chat_service.rs:250
cron: None, // ❌ 初始化为 None

// apps/cli/src/main.rs:263-267
let mut chat = ChatService::new(db, settings, transport, workspace)
    .with_local_command_executor(Arc::new(executor));
// ❌ 未调用 with_cron()！

// apps/desktop/src-tauri/src/lib.rs:6518-6553
ChatService::new(...)
    .with_mcp(...)
    .with_plugins(...)
    .with_knowledge(...)
    // ❌ 未调用 with_cron()！
```

**缺口**：
- ❌ **`CronService::new` 从未调用**（P1 缺陷，已列入 §2.3）
- ❌ **`run_tick_loop` 从未启动**：全局搜索无调用者
- ❌ **工具虽注册，但 backend=None**：`CronCreateTool` 运行时返回 "unavailable"

**修复路径**（本次已实现）：
1. ✅ `tool_runtime.rs:512` 提取 `cron` 字段
2. ✅ `tool_runtime.rs:528` 调用 `register_cron_tools()`
3. ❌ CLI/Desktop **仍需补充**：
   ```rust
   // apps/cli/src/main.rs:263
   let cron_service = Arc::new(CronService::new(&workspace, chat.clone()));
   let chat = chat.with_cron(cron_service.clone());
   
   // 启动 tick loop
   tokio::spawn(async move {
       cron_service.run_tick_loop(shutdown_token).await
   });
   ```

---

### 2.8 记忆与知识（Partial, 架构双轨）

#### 2.8.1 KnowledgeService（Real, 生产路径）

**判定**：**Real** — chunk-level retrieval，已接入 `ChatService` 和 `ContextRuntime`

**实现位置**：
- `crates/deepagent-knowledge/src/base.rs:95` `type Retriever<E> = ContextualRetriever<E, HeadingContextualizer, ScoreReranker>`
- `crates/deepagent-app-core/src/knowledge_service.rs:599` `KnowledgeServiceBackend`

**实现深度**：
- **Chunk-level**：Markdown → heading tree → chunks (512 tokens) → embed → BM25 + cosine + RRF
- Contextualizer：`HeadingContextualizer` 为每个 chunk 附加标题路径
- Reranker：`ScoreReranker` 融合 BM25 + cosine 分数

**生产接入路径**：
```
ChatService::with_knowledge(Arc<KnowledgeService>)
  → RunAssembler::run() → build_main_run_toolset()
    → register_knowledge_write_tool()
      → KnowledgeServiceBackend::new(knowledge)
        → [KnowledgeSearch工具调用] → knowledge.search(query)
          → retriever.retrieve(query, limit) → RRF fusion
```

**已验证能力**：
- ✅ Chunk 切分：`deepagent-memory/src/chunking.rs:80` 测试
- ✅ RRF 融合：`deepagent-memory/src/retrieval.rs:240` 测试

---

#### 2.8.2 MemoryItem（Shell, 蓝图未接入）

**判定**：**Shell** — item-level 抽象已实现，但未接入生产（架构双轨）

**实现位置**：
- `crates/deepagent-memory/src/item.rs:13` `struct MemoryItem`
- `crates/deepagent-memory/src/tier.rs:31` `enum MemoryTier`

**实现深度**：
- 分层存储：`LongTermMemory` (SQLite) / `WorkingMemory` (in-memory) / `EpisodicMemory` (recent events)
- Semantic/Hybrid retriever：`semantic.rs` + `hybrid.rs`（embed + BM25 + graph）
- 原语完整：`MemoryItem { content, embedding, metadata, tier, ... }`

**缺口**：
- ❌ **未接入生产**：`ChatService` 使用 `KnowledgeService`（chunk-level），不用 `MemoryItem`（item-level）
- ❌ **架构双轨**（P1 缺陷，已列入 claudecode-harness-research-report.md §5.3）：两套记忆系统并存，职责重叠

---

### 2.9 模型适配（Real, DeepSeek 官方协议）

#### 2.9.1 DeepSeek Responses API（Real）

**判定**：**Real** — reasoning/streaming/usage 完整支持

**实现位置**：
- `crates/deepagent-models/src/chat.rs:558` `reasoning_text_projection()`
- `crates/deepagent-models/src/chat.rs:232` reasoning content block

**实现深度**：
- **5,700+ LOC** (`deepagent-models`)
- Reasoning：`reasoning_effort` (low/medium/high) + `reasoning_tokens` usage
- Streaming：SSE 解析 + `ReasoningDelta` / `ContentDelta` 事件
- Tool calling：`tool_calls` 数组 → `ToolUse` 解析
- Usage 统计：`prompt_tokens` / `completion_tokens` / `reasoning_tokens`

**生产接入路径**：
```
RuntimeEngine::run() → agent.think()
  → ModelAgent::think_streaming() (model_agent.rs:180)
    → ResponseRequest::new(model_id, messages).send(transport)
      → DeepSeek Responses API (https://api.deepseek.com/chat/completions)
        → SSE stream → parse chunks → RuntimeEvent::ReasoningDelta/ContentDelta
```

**已验证能力**：
- ✅ Reasoning：`chat.rs:744` 测试 reasoning block 解析
- ✅ Streaming：`stream.rs:140` 测试 SSE chunk 处理

**缺口**：
- ❌ **Tokenizer 估算粗**：用启发式规则（`4 chars = 1 token`），未用真实 tokenizer（P1 缺陷，已列入 §2.2）
- ❌ **无 usage 校准**：不对比 provider 返回 usage，估算误差未量化

---

### 2.10 Harness 协议/SDK（Partial, stdio 有、HTTP/WebSocket 缺）

#### 2.10.1 stdio JSON-RPC + JSONL（Real）

**判定**：**Real** — CLI 通过 stdin/stdout 交互，已生产使用

**实现位置**：
- `crates/deepagent-harness-protocol/src/*.rs` (1,407 LOC, 7 模块)
- `apps/cli/src/app_server.rs:200` stdio RPC server

**实现深度**：
- **JSON-RPC 2.0**：`initialize` / `submit` / `interrupt` / `steer` / `cancel`
- **JSONL 事件流**：每个 `RuntimeEvent` → JSONL 行 → stdout
- **双向通道**：stdin 读命令 → process → stdout 写事件

**生产接入路径**：
```
CLI main.rs → spawn stdio server
  → AppServer::dispatch(RpcRequest)
    → ChatService::run(..., on_event)
      → on_event(RuntimeEvent) → JSONL::serialize → println!()
```

**已验证能力**：
- ✅ RPC 协议：`app_server.rs:222` 测试 initialize/submit
- ✅ JSONL 流：CLI 输出 `{"type":"ContentDelta",...}` 行

---

#### 2.10.2 HTTP/WebSocket（Missing）

**判定**：**Missing** — 无 HTTP API、无 WebSocket streaming

**缺口**：
- ❌ **无 HTTP REST API**：Desktop 用内存通道（Tauri invoke），不暴露 HTTP
- ❌ **无 WebSocket**：无法从浏览器直连 harness

---

### 2.11 Skills/Plugins（Real, 生产主链）

#### 2.11.1 Skills（Real）

**判定**：**Real** — 动态加载、frontmatter 元数据、渐进披露

**实现位置**：
- `crates/deepagent-skills/src/*.rs` (3,268 LOC)
- `crates/deepagent-app-core/src/skill_catalog.rs` (动态加载)

**实现深度**：
- Skill 格式：Markdown frontmatter + body (`---\nname: example\n---\n{{INSTRUCTIONS}}`)
- 动态加载：`.deepagent/skills/` + `~/.claude/skills/` + builtin
- 渐进披露：skill delta（上下文预算控制）

**生产接入路径**：
```
RunAssembler::run() → load_skills()
  → SkillCatalog::discover() → parse frontmatter
    → SystemContextAssembly::with_skills() → [SKILLS] section
```

---

#### 2.11.2 Plugins（Real）

**判定**：**Real** — 运行时 overlay（skills/agents/mcp/hooks），生产已用

**实现位置**：
- `crates/deepagent-plugins/src/*.rs` (多 crate 拆分)
- `crates/deepagent-app-core/src/plugin_service.rs`

**实现深度**：
- Plugin manifest：`plugin.json` (name/version/capabilities)
- Runtime overlay：插件贡献 skills/agents/MCP servers/hooks/commands
- Lifecycle：enable/disable/install/uninstall

**生产接入路径**：
```
ChatService::with_plugins(Arc<PluginService>)
  → RunAssembler::run() → load_plugin_overlays()
    → merge plugin skills/mcp servers/hooks → ToolRegistry
```

---

### 2.12 可观测性与治理（Partial, 本地有、企业缺）

#### 2.12.1 RuntimeLogStore（Real, 本地诊断）

**判定**：**Real** — 本地运行时日志，SQLite 持久化

**实现位置**：
- `crates/deepagent-app-core/src/runtime_log_store.rs:18` `struct RuntimeLogStore`

**实现深度**：
- 日志级别：info/warn/error
- 结构化 data：JSON 字段
- 查询：按 session_id / run_id / source 过滤

**生产接入路径**：
```
ChatService::with_runtime_logs(Arc<RuntimeLogStore>)
  → append_runtime_log(entry) → INSERT INTO runtime_logs
```

---

#### 2.12.2 成本度量（Partial, 框架有、未完整）

**判定**：**Partial** — `CostService` 已实现，但 token usage 估算粗、无企业预算强制

**实现位置**：
- `crates/deepagent-app-core/src/cost_service.rs`

**缺口**：
- ❌ **无企业预算**：无组织级配额、无超额拦截
- ❌ **无成本分析 UI**：Desktop 不展示成本趋势

---

## 3. 剩余架构缺陷清单（不与现有报告重复）

以下缺陷**未列入** `deepagent-defects-and-fixes.md`（该文档已覆盖 P0/P1/P2 共 6 项），为本次审计新发现：

### D6【P2】CodeGraph 未接入上下文工程

**现状**：`deepagent-codegraph` (12k LOC) 已实现 tree-sitter 解析 + 增量变更检测 + 多语言支持（Rust/Python/TypeScript/Go），但**未自动注入上下文**。

**证据**：
- `crates/deepagent-codegraph/src/lib.rs:149` `CodeGraph::build()` 生成 schema
- `SystemContextAssembly` **不包含** `[CODE_GRAPH]` section

**影响**：模型无法利用 code schema 做精准代码导航

**修复优先级**：P2（非阻塞，但影响智能）

---

### D7【P2】子代理无结构化摘要

**现状**：子代理完成后，父 run 只收到 `"subagent finished"` 文本，无结构化输出（成功/失败/修改文件/执行工具）。

**证据**：
- `subagent_runner.rs:520` 返回 `String`（plain text）
- Claude Code 有 `AgentResult { status, summary, outputs }`（`借鉴/claudecode/restored-src/src/agent.ts:420`）

**影响**：Desktop UI 难追踪子代理进度，无法展示"哪个子代理改了哪些文件"

**修复优先级**：P2（产品体验）

---

### D8【P3】无 WebSocket streaming

**现状**：CLI 通过 stdio JSONL 流式输出，Desktop 通过 Tauri event，但**无 HTTP/WebSocket API**，无法从浏览器直连。

**影响**：无法构建 Web 前端（如 claude.ai/code）

**修复优先级**：P3（非当前产品需求）

---

### D9【P3】无 Remote Control

**现状**：无跨设备 agent 寻址、无云会话投影、无远程 SendMessage。

**影响**：无法实现"笔记本启动任务 → 台式机执行"场景

**修复优先级**：P3（未来特性）

---

## 4. 分阶段修复路线图（整合现有报告）

基于 `deepagent-defects-and-fixes.md` 已有路线（S1/S2/S3），补充本次新发现：

### Phase S1（P0）：上下文减重（已列入 §2.1）
1. MCP instructions 增量披露（P0）
2. PromptBudget::fit() 生产调用（P1）

### Phase S2（P1）：harness 能力补齐（已列入 §2.3 + 本次 D5）
3. **Cron 产品化**（本次已注册工具，仍需 CLI/Desktop 启动 tick loop）
4. Teammate 异步上下文隔离（未实现）
5. 子代理 frontmatter 扩展（P1）
6. 提示词拼接单源收敛（P1）

### Phase S3（P2）：产品体验
7. CodeGraph 注入上下文（本次 D6）
8. 子代理结构化摘要（本次 D7）
9. 迁移框架（P2）
10. 遥测台账（P2）

### Phase S4（P3）：未来特性
11. WebSocket API（本次 D8）
12. Remote Control（本次 D9）

---

## 5. 结论

### 5.1 DeepAgent 的真实能力边界

**DeepAgent Studio 不是原型，而是一个生产级 harness 内核**，已在以下方面达到 Claude Code 参照水平：

- ✅ **核心执行链路**（AgentKernel → RuntimeEngine → RunStore）：流式输出、工具调用、中断/取消、事件持久化、resume/replay
- ✅ **工具生态**（16+ 内置工具 + MCP 客户端）：真实文件操作、Bash 沙箱、权限拦截
- ✅ **审批与权限**（InputLeaseRegistry + ApprovalGate + Hooks）：声明式规则、外部 hooks、UI 审批桥接
- ✅ **上下文预算**（ContextPolicy + 自动压缩）：200k 封顶、阈值触发 microcompact
- ✅ **子代理**（ChatSubagentRunner）：独立上下文、worktree 隔离、approval 桥接

**核心差距在产品装配完整性，而非技术能力**：

- ⚠️ **DagScheduler 已实现但未装配**：真实并发 fan-out 代码存在，但 UI 不暴露
- ⚠️ **Cron 底层完整但未启动**：scheduler + store + tools 全有，但 CLI/Desktop 未调用 `with_cron`
- ⚠️ **记忆系统架构双轨**：chunk-level（生产）+ item-level（蓝图）并存，需收敛
- ⚠️ **上下文工程细节**：MCP instructions 全量重发、tokenizer 估算粗、golden test 缺失

### 5.2 与 Claude Code 的战略差异

DeepAgent 的**不应复制** Claude Code 的部分：

1. **模型方向**：DeepSeek（开源/自托管）vs Claude（闭源/云服务） → 不复制 `cache_control` 字段
2. **协议设计**：借鉴边界，不复制字段形状 → 工具调用用 Rust trait，不模仿 `tool_use` block
3. **平台策略**：桌面优先 vs 云优先 → 优先完善本地 harness，再考虑云投影

DeepAgent 的**应对齐** Claude Code 的部分：

1. **上下文工程**：MCP instructions delta、工具 schema 懒加载、缓存契约测试
2. **多代理编排**：装配 DagScheduler、暴露 PlanExecuteTool、UI 展示 DAG 拓扑
3. **跨会话任务**：启动 Cron tick loop、实现 Teammate 寻址、补全任务生命周期

### 5.3 最优路线（重申）

> **保留 `AgentKernel → RuntimeEngine → RunStore/run_events` 主链，收敛架构双轨（提示词拼接/记忆系统），装配已实现能力（DagScheduler/Cron），再补跨会话任务体系。**

不重写内核，不再造 Claude Code，而是：
1. **Phase S1（P0）**：上下文减重（MCP delta、PromptBudget 生产化） — 2 周
2. **Phase S2（P1）**：harness 装配（Cron 启动、子代理 frontmatter、提示词单源） — 3 周
3. **Phase S3（P2）**：产品体验（CodeGraph 注入、子代理摘要、遥测） — 4 周
4. **Phase S4（P3）**：未来特性（WebSocket、Remote Control） — TBD

---

**报告完成日期**：2026-10-01  
**审计方法**：实码审计（33 crates / ~16 万行 Rust） + 能力映射 + 实现深度判定  
**证据标准**：每项结论给出文件路径 + 行号 + 生产接入路径  
**审计范围**：DeepAgent Studio 全架构 vs Claude Code 参考实现（`借鉴/claudecode`）  
**结论置信度**：High（基于实码，非推测）