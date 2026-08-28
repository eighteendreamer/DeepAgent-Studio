# DeepAgent Studio 与 Warp 全方面深度对比报告

> 调研对象：`G:\Code_Warehouse\DeepAgent-Studio\借鉴\warp`
>
> 调研快照：Warp `061318ff7fc424e41fbd77e30432995d483c99e4`（2026-08-28）；DeepAgent Studio 当前工作树。
>
> 证据规则：本文只把源码、测试、Cargo workspace、项目规则和明确注释视为“已实现”。Warp 的 `specs/`、资源文件、feature flag 或服务端 API 依赖只能标记为“设计/依赖”，不冒充本地闭环。由于本次官方网页检索服务返回 502，本文不把网页宣传语作为事实来源。

## 1. 结论摘要

Warp 的强项不是单个 agent loop，而是把“终端作为 agent 工作台”做成了产品平台：GUI/TUI 共用 `warp_core`/WarpUI Entity 模型；`ai::agent` 负责服务端对话数据和富输出；`ai::blocklist` 负责动作预处理、队列、并发、阻塞和用户确认；`ai::agent_sdk::AgentDriver` 负责 CLI、环境、MCP、skill、provider、harness-support 和输出格式；ambient agents、orchestration、remote server、multi-agent API、Computer Use 又把单机运行扩展到云端、远端和多代理协作。

DeepAgent Studio 的方向相反但更适合作为“运行时内核”：`AgentKernel`/`RuntimeEngine` 是唯一执行链，`RunStore`/`EventStore` 是持久化真相，`RuntimeEvent` 再投影为版本化 JSON-RPC harness 事件；工具、审批、hook、取消、checkpoint、verification、MCP、SandboxBackend 都在可组合 seam 上。就“平台化、可 replay、可跨 CLI/SDK/Desktop 复用、可测试”而言，DeepAgent 的边界更清楚；就“真实终端交互、云端任务管理、并行 child agent、成熟的多表面体验、环境准备和生态产品化”而言，Warp 更深。

最重要的判断：不要复制 Warp 的第二条执行链。应吸收 Warp 的产品能力与状态细节，投影进现有 `AgentKernel -> RuntimeEngine -> RunStore/EventStore -> harness protocol`，优先补四个缺口：

1. 将动作级状态机（preprocess/queued/blocked/running/finished）提升为运行时标准事件和可恢复状态。
2. 将本地、远程、后台、定时、child agent 统一成带 parent/child、location、join/resume 的 run graph。
3. 将 environment snapshot、skill/MCP 生命周期、artifact、usage/cost、provider fallback 纳入协议和持久化。
4. 为 TUI/PTY、终端 takeover、Computer Use、远程执行定义统一 capability/approval/sandbox contract，而不是让入口层各自解释。

## 2. 对比边界与证据清单

### 2.1 产品与代码规模

| 项目 | Warp | DeepAgent Studio | 判断 |
|---|---|---|---|
| 主产品 | Rust 终端模拟器 + GUI/TUI + Agent Mode + Cloud/Ambient Agent | Rust agent runtime + Tauri Desktop + headless CLI + SDK | 定位不同，不能只用“agent 功能数”比较 |
| workspace | 根 `Cargo.toml` 包含 `app`、`warp_core`、`warpui`、`warp_tui`、`ai`、`ai_types`、`warp_server_client`、`warp_multi_agent_client`、`remote_server`、`computer_use`、`isolation_platform` 等 | 根 `Cargo.toml` 明确拆成 core、runtime、models、tools、mcp、persistence、app-core、harness-protocol、security、subagents 等 20+ crate | Warp 是产品单体化 workspace；DeepAgent 是内核平台化 workspace |
| GUI/TUI | `借鉴/warp/AGENTS.md` 明确说明 GUI 与 headless TUI 共享 Entity/model core，渲染和输入分叉 | Desktop 前端消费 Tauri command/event；CLI 和 SDK 消费 app-server harness | Warp 的终端表面成熟；DeepAgent 的机器边界更成熟 |
| 当前快照 | git commit `061318ff...` | 当前仓库 `main`，工作树初始无未提交变更 | 结果针对快照，不代表所有线上 Warp 服务实现 |

### 2.2 关键证据文件

| 能力 | Warp 源码证据 | DeepAgent 源码证据 |
|---|---|---|
| 对话/输出 | `借鉴/warp/app/src/ai/agent/mod.rs`、`conversation.rs`、`task.rs`、`api.rs` | `crates/deepagent-session`、`crates/deepagent-runtime/src/kernel.rs`、`crates/deepagent-runtime/src/events.rs` |
| 动作队列/审批 | `借鉴/warp/app/src/ai/blocklist/action_model.rs` | `crates/deepagent-runtime/src/tool_pipeline.rs`、`crates/deepagent-runtime/src/approval.rs` |
| SDK/harness | `借鉴/warp/app/src/ai/agent_sdk/driver.rs`、`harness_support.rs`、`output.rs` | `apps/cli/src/app_server.rs`、`packages/sdk/src/index.ts`、`crates/deepagent-harness-protocol` |
| orchestration/ambient | `借鉴/warp/app/src/ai/orchestration/`、`ai/ambient_agents/`、`pane_group/child_agent/` | `crates/deepagent-subagents`、`crates/deepagent-persistence/src/subagent_store.rs`、runtime subagent events |
| MCP/skills/context | `借鉴/warp/app/src/ai/mcp/`、`ai/skills/`、`crates/ai/src/project_context/` | `crates/deepagent-mcp`、`crates/deepagent-skills`、`crates/deepagent-context`、`crates/deepagent-knowledge` |
| 安全/隔离 | `借鉴/warp/crates/isolation_platform/`、`ai` 的 autonomy/isolation 参数、终端 bootstrap | `crates/deepagent-app-core/src/sandbox_backend.rs`、`crates/deepagent-tools/src/permission.rs`、`sandbox.rs`、`crates/deepagent-security` |
| 测试/可观测 | Warp `app/src/ai/*_tests.rs`、`integration_testing/agent_mode`、TUI render tests、Sentry/OpenTelemetry 依赖 | runtime/kernel/tool-pipeline/chat-service/harness protocol 回归测试，runtime log/cost/metrics stores |

## 3. 总体架构对照

### 3.1 Warp：终端产品平台架构

```text
GUI WarpUI / Headless TUI
        |
warp_core App/Entity/Model + Terminal/PTY/Workspace
        |
ai::agent (Conversation/Task/Exchange/Output/Action/Context)
        |
ai::blocklist ActionModel + ActionExecutor
        |       |-- preprocess -> queue -> blocked/approval
        |       |-- sync/async execution -> result -> next API request
        |
ai::agent_sdk::AgentDriver
        |-- terminal bootstrap / environment clone / skills / MCP / provider
        |-- harness support / JSON / NDJSON / third-party CLI sessions
        |
warp-server GraphQL/WebSocket + multi-agent API + remote server + ambient/orchestration
```

特点：对话的权威状态主要是 server API 返回的 `Task`/`AIAgentOutput`，客户端通过 `ServerConversationToken`、`ServerOutputId` 和 proto response events 关联；动作状态是 UI/执行器内部模型；远端/云端 task 的持久状态在 Warp 服务端。

### 3.2 DeepAgent Studio：运行时内核平台架构

```text
CLI app-server / TypeScript SDK / Tauri Desktop
        |
HarnessRequest/Response + HarnessEvent (协议 DTO)
        |
ChatService / AppCore (组装 provider, registry, MCP, permissions, sandbox)
        |
AgentKernel
        |-- RunStore: run phase/state/terminal/event append
        |-- RuntimeEngine: think -> model stream -> tool pipeline -> verify -> finalize
        |-- PersistentEventSink: RuntimeEvent 持久化 + live sink
        |
RunStore/EventStore/Session/Checkpoint/Artifact/Cost/RuntimeLog
```

特点：事件日志是 source of truth，live sink 只是投影；`RunPhase` 明确为 Accepted、Preparing、RunningTurn、ExecutingTools、Verifying、Finalizing、Terminal；终态映射集中在 `AgentKernel`，不会让 UI、CLI、SDK 各自解释。

### 3.3 架构结论

Warp 更像“完整 agent 产品”，DeepAgent 更像“可外接多个产品的 agent runtime”。Warp 的功能广度目前领先，但跨入口一致性依赖 server/client task、UI model、CLI driver 和多套 transport；DeepAgent 的扩展性和审计一致性领先，但需要补足 Warp 已验证的终端和云端运营能力。

## 4. Harness 流程逐步对照

### 4.1 Warp 本地 Agent Mode 流程

1. 用户在 GUI/TUI/终端输入 prompt，创建或恢复 `AIConversationId`，已有对话通过 `ServerConversationToken` 续接。
2. `AIAgentApi::RequestParams` 汇总输入、conversation token、tasks、session context、model、coding/CLI/computer-use model、MCP context、planning、web search、autonomy、isolation、feature flags、BYOK 和 parent agent。
3. API 请求发给 Warp server；response stream 返回富事件。`AIAgentOutput` 保存 text、reasoning、actions、todos、subagent、skill、artifact、citations、model info、cache expiry、cost、telemetry。
4. action 输出进入 `BlocklistAIActionModel`。动作先 `Preprocessing`，再按原始顺序进入 per-conversation `VecDeque`；队首在没有运行 action 时变成 `Blocked`，后续 action 为 `Queued`。
5. 预处理可以并行，但 `action_order` 保留 server 原顺序；这允许读取/搜索等安全动作并发，同时保持回传顺序稳定。
6. 队首动作经 `ActionExecutor::try_to_execute_action`，可同步完成、异步运行，或因权限/前置条件返回 NotExecuted 并保留在队列。
7. 用户确认、拒绝、修改命令、取消、takeover、long-running command 等路径都转化为 `AIAgentActionResult`；结果释放 executor-held state，再把 action result 加入下一次 API query。
8. 若动作需要持续监控，CLI subagent/remote session 负责 PTY，主 agent 可能被取消后以 `CancellationReason::CLISubagentUserTakeover` 或 `CommandFinishedDuringInlineAgentView` 恢复。
9. stream 完成时产生 `FinishedAIAgentOutput::{Success,Cancelled,Error}`；取消原因映射到 `CancellationOutcome`，包括 keep-in-progress、succeeded、cancelled、externally-finalized。

证据：`ai/agent/mod.rs:82-142,220-365,441-469,1042-1057,1399-1432,1728-1755`；`ai/blocklist/action_model.rs:76-125,224-258,625-667,900-951,971-1048`。

### 4.2 Warp Agent SDK/Cloud/Ambient 流程

1. `AgentDriverOptions` 提供 working directory、secrets、task id、parent run id、共享 session、harness 类型和输出设置。
2. `run_internal` 先检查 cwd，等待 terminal bootstrap，再做 cloud provider setup；这一步明确解决 MCP 依赖 PATH/凭据的竞态。
3. 解析/启动 managed、file-based、builtin、ephemeral MCP；收集降级信息。按 harness 解析 global/environment skills，可 clone repository，记录 environment snapshot 和 resolved HEAD。
4. 运行 preflight、setup harness/plugins/notification/platform plugin，校验所需插件，配置 terminal、base model override、provider credentials。
5. `run_harness`/`execute_run` 启动 Oz 或第三方 CLI harness；事件写入 task/exchange，并以 `OutputFormat::{Pretty,Text,Json,Ndjson}` 输出。
6. driver 维护 idle timeout、credential refresh、retry、错误分类、linger-after-failure、snapshot upload 和 cleanup。
7. `warp harness-support` 通过 run id 关联 workload token，提供 ping、report-artifact、report-external-reference、notify-user、finish-task、report-shutdown；这些命令是“agent 内部向平台回报”的 side-channel。

证据：`ai/agent_sdk/driver.rs:501-727,1220,2757,3454-3828,4032`；`ai/agent_sdk/driver/environment.rs:38-234,371-536,779-1175`；`ai/agent_sdk/harness_support.rs:23-319`。

### 4.3 DeepAgent Studio harness 流程

1. app-server 收到 `initialize`、`thread/start` 或 `thread/resume`，再接受 `turn/start`；`turn/interrupt`、`turn/steer`、`approval/respond` 与 `tool/list`、`config/read`、`sandbox/status` 共用协议。
2. `ChatService` 组装 session、model client、ToolRegistry、MCP、hooks、permissions、sandbox executor、skills、knowledge、verification、cost 和 runtime broker。
3. `AgentKernel::start_prepared` 创建 run record，写入 `run_accepted`，推进 Preparing，创建 `PersistentEventSink`，把 registry、cancel、events、approvals、hooks、verification 注入 `RuntimeEngine`。
4. RuntimeEngine 执行 think/stream/tool loop；model stream 保留 reasoning/content、usage、DeepSeek raw response usage 和 tool-call 增量。
5. `ToolExecutionPipeline::prepare` 进行 schema validation、hook、permission/risk、approval、checkpoint 与 artifact 预处理；只有允许的 invocation 才进入 `execute_prepared`。
6. tool 结果、hook、approval、subagent、checkpoint mutation、verification、usage 等都发为 `RuntimeEvent`，同时由持久化 sink 写入事件存储并推送 live sink。
7. run outcome 统一映射为 Succeeded、Cancelled、Blocked、MaxTurns、BudgetExceeded、Failed、ContextExhausted；只写一次 terminal event/finish。
8. harness 层通过 `project_runtime_event` 把 runtime event 转成 `thread.*`、`turn.*`、`item.*`、`approval.requested`、`turn.interrupted` 等稳定事件；`ThreadRead(afterSequence)` 支持断线续读和 replay。

证据：`deepagent-runtime/src/kernel.rs:24-81,101-124,318-357,398-447,470-546`；`deepagent-harness-protocol/src/requests.rs:9-175`；`harness-protocol/src/events.rs:21-80,126-190,195-315,400-418`。

### 4.4 流程差异表

| 步骤 | Warp | DeepAgent Studio | 差异影响 |
|---|---|---|---|
| 输入入口 | GUI/TUI/terminal/CLI agent/cloud task 多入口 | app-server/SDK/Tauri/CLI，多入口但共用 ChatService | DeepAgent 更容易保证协议一致 |
| 会话主键 | server conversation token + client task/exchange | thread/session + run/turn + sequence | Warp 远端关联强；DeepAgent replay 语义强 |
| 模型循环 | server response stream 驱动客户端 action loop | 本地 RuntimeEngine 驱动 model/tool loop | Warp 利于云端托管；DeepAgent 利于自部署/可测试 |
| action 排队 | 明确 per-conversation 队列和原序恢复 | pipeline 单次 invocation，跨 turn 状态依赖 RunStore/engine | DeepAgent 需补动作级可恢复队列 |
| 审批 | action blocked 后 UI card/selector；view-only 会跳过交互 | ApprovalGate 返回 allow/deny/awaiting，协议发 approval.requested | DeepAgent 协议清楚，Warp 交互和复杂场景更成熟 |
| 取消 | 多种业务取消原因和 cloud handoff/takeover | CancellationTree + 统一 terminal classification | Warp 语义更细，DeepAgent 终态更集中 |
| 远程/后台 | Ambient task、cloud run、remote child、scheduled | subagent/worktree/scheduler crate，主要是本地 runtime | Warp 运营平台更完整 |
| 恢复 | conversation restore、task resume、server token | thread resume、event replay、checkpoint | 两者都有；DeepAgent 事件恢复边界更明确 |

## 5. 架构点逐项对比

### 5.1 核心抽象与依赖方向

Warp 的核心抽象是 `App`/Entity/Model/Handle、`AIAgentOutput`、`AIAgentAction` 和 server proto。AI、terminal、UI、persistence、server client 之间存在大量 app 层交叉引用，优点是产品联动快，缺点是把机器协议、UI 视图状态和 server API 类型混在同一产品树中。

DeepAgent 以 trait 和 crate seam 分层：`Agent`、`ModelClient`、`Tool`、`ToolRegistry`、`ApprovalGate`、`RuntimeEventSink`、`SandboxBackend`、`McpTransport`、`SubAgentExecutor`。`app-core` 负责装配，runtime 不依赖 Tauri/React。应保持这一方向，不把 Warp 的 UI model 直接搬进内核。

### 5.2 会话、任务、turn、exchange

Warp `Task` 同时表达 root task、parent task、subagent 类型、exchange 列表、message/context、working directory 和 server source；`Task` 的 `parent_id`、`is_cli_subagent`、`is_advice_subagent`、`is_computer_use_subagent` 让同一模型支持多种子代理。`AIAgentExchange` 以 server message 为中心，结果必须回填下一次 API 请求。

DeepAgent 的 `Session`、`TaskId`、`RunId`、`TurnId`、事件 sequence 已拆开；这更适合独立恢复和多客户端，但需要一个持久化 run graph 来承载 Warp 的 parent/child task、execution location、joinable session 和 follow-up 语义。

建议：保持 `Session -> Run -> Turn -> Item/ToolCall` 的协议层级，新增 `parentRunId`、`executionLocation`、`joinUrl/sessionId`、`background`、`resumeToken`，不要把所有语义重新塞入 Session。

### 5.3 状态机与并发

Warp 的 `AIActionStatus` 有五态：Preprocessing、Queued、Blocked、RunningAsync、Finished(result)。批次预处理并行，动作执行按队列和交互约束串行/并行混合；`action_order` 保障 response 顺序。这个状态机对“多个工具调用同时到达，但只有一个需要用户确认”的场景很有价值。

DeepAgent 的 run phase 是宏观生命周期，tool pipeline 有 prepare/execute/complete 三段，但目前没有 Warp 那样持久化的 per-action queue 状态。应增加 `ToolInvocationState`（prepared/queued/blocked/running/completed/failed/cancelled）及 action sequence，并在 `RuntimeEvent` 中携带 `item_id/call_id`。

### 5.4 模型与 provider

Warp `RequestParams` 同时区分主模型、coding model、CLI agent model、computer-use model，支持 BYOK、custom provider/router、Warp credits、fallback、prompt cache expiry；`OutputModelInfo` 记录最终 model 与 fallback。provider 还可能在 SDK driver 中以 cloud credential env 注入。

DeepAgent `deepagent-models` 已有 `ThinkingDepth/ThinkingConfig`、tool schema、usage、finish reason、SSE accumulator、raw usage、capability resolver，且 `RuntimeEvent` 保留 DeepSeek reasoning/cache 信息，provider adapter 边界更干净。尚需补：模型 profile 组合（主/编码/Computer Use）、fallback chain、provider credential lifecycle、cache expiry、按 provider/model/category 的 cost aggregation。

### 5.5 上下文、项目索引、记忆

Warp 有 project context 规则分层、full-source-code embedding、remote search、Warp Drive context、conversation search、memory store API、global/environment skills；AgentDriver 会在运行前 clone repos、解析 HEAD、建立 codebase index。

DeepAgent 已拆 `context`、`knowledge`、`memory`、`codegraph`、`skills`，并在 ChatService 中按 session 发送 skill/tool；模型 loop 也有 context overflow terminal。差距在于“运行前环境快照 + 多 repo 固定提交 + 索引状态 + server/cloud memory”尚未形成统一 run artifact。应把 snapshot、repo SHA、index status、selected rules、memory references 写入 run metadata。

### 5.6 工具系统与 MCP

Warp 的 action 类型直接覆盖 file read/edit、command output、grep/glob、search codebase、MCP resource/tool、web search/fetch、artifact、question、subagent、todo、computer use；MCP manager 支持 file-based、templatable、builtin、CLI-spawned、OAuth、资源和工具索引。

DeepAgent `Tool`/`ToolRegistry`/schema validation 与 `deepagent-mcp` transport/registry/adapter 已把协议边界做对；`ToolExecutionPipeline` 还串 hook、approval、checkpoint、artifact。功能差距主要是 MCP installation lifecycle、OAuth/ephemeral server、MCP degradation reporting、tool discovery snapshot 和资源引用，而不是基础调用能力。

### 5.7 审批、权限和安全

Warp 通过 autonomy level、isolation level、team policy、view-only、action blocked UI 和 server-side task policy 管理权限；终端命令、文件编辑、MCP 和 Computer Use 分别有交互卡。它的优势是用户体验细，风险是规则来源分布在 server request params、blocklist、UI 和 terminal。

DeepAgent 有显式 `PermissionSet`/`RiskLevel`、`ApprovalGate`、hook deny/ask、command guard LLM、secret scanner/redaction、permission profile 协议字段，且测试覆盖 invalid schema、high-risk approval、auto-review/full-access。建议借鉴 Warp 的“动作前置展示与可编辑命令”，但审批判断必须继续集中在 pipeline/gate。

### 5.8 沙箱、PTY、远程和 Computer Use

Warp 原生拥有 terminal bootstrap、PTY、shell integration、remote SSH/server、cloud environment/runner、isolation platform、Computer Use screenshot/mouse/recording。它把 agent 的现实执行面做深了。

DeepAgent 已有 `SandboxBackend::{Direct,Sandboxie,WindowsSandbox}`、网络策略、写入 workspace 控制、`.wsb` task plan、artifact collection、SSH crate、vision service；但真实 PTY takeover、跨机 remote worker、Computer Use action protocol 仍不是 Warp 同量级的产品闭环。这里不应只加一个“computer_use 工具”，而要先定义 capability、approval、input lease、screen artifact、replay 和 cancellation contract。

### 5.9 子代理、编排、后台与定时

Warp 的 `SubagentType` 包含 CLI、Research、Advice、ComputerUse、Summarization、ConversationSearch、WarpDocumentationSearch；orchestration 维护 host/model/environment/auth selection；ambient task 维护 creator/executor、execution location、live session、credits、cancellable/terminal state；scheduled manager 负责创建、暂停、恢复、更新、删除。

DeepAgent `SubAgentContext/Result/Executor`、worktree provider、DAG scheduler、subagent store 与 runtime subagent events 已有正确骨架，但任务图、后台 join、远程 location、schedule 持久化和 parent cancellation 需要完善。优先把 child run 作为同一 RunStore/event protocol 的子 run，而不是单独造第二个 event store。

### 5.10 持久化、replay、审计

Warp 本地 persistence 保存 conversation/task/exchange/block metadata，server 保存 cloud task；driver 另写 exchange input/output，server token 负责跨服务关联。它的 replay 依赖 conversation restore 和 proto message conversion，复杂度高但富输出保真。

DeepAgent `RunStore` 明确 append event、transition、finish、events_after；`EventStore` 支持 session read/fork/truncate；`PersistentEventSink` 保证 live projection 不成为真相；secret redaction 和 raw usage 也有明确路径。这是 DeepAgent 的最大结构优势。

### 5.11 事件与协议

Warp 的内部事件覆盖 response event、action result、blocklist event、terminal driver event、ambient event、orchestration event，跨边界主要是 GraphQL/WebSocket/proto 和 CLI JSON/NDJSON。它能表达的产品事件很丰富，但没有一个与 `thread/start` 等价的统一公开本地协议。

DeepAgent `HarnessRequest` 已覆盖 initialize、thread 生命周期、turn start/interrupt/steer、approval、tool/config/sandbox；`HarnessEvent` 有稳定的 thread/turn/item/approval/error/interrupt 投影，并带 protocol version、threadId、turnId、itemId、afterSequence。缺口是把更多 runtime 事件（hook、checkpoint mutation、environment snapshot、artifact、provider retry、MCP lifecycle、child join）从 generic runtime item 提升为一等协议事件。

### 5.12 CLI、SDK、桌面与交互

Warp TUI 是 agent 体验的一等前端，拥有 option selector、permission card、ask-question、orchestration config、focus synchronization、render-to-lines 测试；CLI agent session 还可接 Claude/Codex/Gemini/OpenCode 等第三方 harness。

DeepAgent Desktop 的 Tauri bridge 已有 `start_chat_v2`、事件通道、审批通道和完成通知，TypeScript SDK 已消费 harness JSON-RPC；但 TUI/PTY UX、第三方 CLI session plugin、session sharing/view-only 等体验需要补齐。协议 DTO 与 UI DTO 的分离应继续保持，Warp 的 UI 结构不要直接成为协议。

### 5.13 可观测性、成本、可靠性和测试

Warp 使用 Sentry、OpenTelemetry、telemetry events、request cost、token usage category、prompt cache expiry、debug payload；driver 有 retry、credential refresh、idle timeout、error classification、linger-after-failure。测试覆盖大量 UI/state/action conversion 和 integration agent mode。

DeepAgent 有 tracing/metrics、runtime logs、cost store、usage event、model attempt reset、stall detector、budget/token/context terminal、verification/adversarial verifier，且 runtime/core/app-core 有回归测试。应吸收 Warp 的运营字段和“setup step 可单独分类”，为每个 run 记录 setup latency、TTFT、model attempts、MCP startup degradation、sandbox startup、artifact upload 和 child cost。

## 6. 技术点覆盖矩阵

| 技术点 | Warp 状态 | DeepAgent 状态 | 结论/行动 |
|---|---|---|---|
| Rust async | Tokio + WarpUI model spawner + callback/update | Tokio runtime，trait/engine async | DeepAgent 边界更简单；需补 UI backpressure |
| UI 架构 | 自研 GPU WarpUI + cell-grid TUI，共享 Entity core | Tauri React + Rust AppCore | Warp 终端交互领先，DeepAgent 跨客户端成本低 |
| 模型流式 | server response event，text/reasoning/action/message hydration | SSE/Responses accumulator、DeepSeek reasoning/cache/raw usage | DeepAgent provider seam 更适合多模型；吸收 Warp 富 event taxonomy |
| tool call | `AIAgentActionType` 大枚举 + `AIAgentActionResultType` | Tool trait + descriptor + validated invocation | DeepAgent 更可扩展；需动作队列状态 |
| 并发 | preprocess 并行，action order 保序，running async | pipeline 控制每次执行，engine loop | 引入 durable action sequence |
| schema | proto/API conversion + action parsing | JSON schema validation 在 pipeline 入口 | DeepAgent 的“先校验再 hook/权限/执行”应保持 |
| hooks | blocklist/terminal/server action hooks | HookRegistry + Before/After/FileChanged 等 HookPoint | DeepAgent 更统一；补 hook 事件协议 |
| approval | UI selector/card + team/autonomy/isolation | ApprovalGate + permission profile + approval/respond | 合并 Warp UX 与 DeepAgent policy seam |
| cancellation | 8+ cancellation reasons + outcome mapping | CancellationTree + terminal classification | 增加业务 reason，但只保留一个终态写入点 |
| PTY/shell | 原生 terminal emulator、bootstrap、takeover、remote TTY | command tool + SandboxBackend + SSH | Warp 深；DeepAgent 需 standalone PTY adapter |
| filesystem | action-specific read/edit/diff/approval | tools + workspace boundary + checkpoint | DeepAgent 审计强；补富 diff/interactive edit |
| MCP | templatable/file/builtin/CLI/OAuth/resource/tool lifecycle | stdio/http/SSE transport + registry/adapter/reconnect | 生命周期和 degradation 是主要缺口 |
| skills | bundled/channel-gated/common skills + global/env repo clone | skill crate + session sent-skill tracking | 补来源、版本、hash、scope、run snapshot |
| context/index | full source embedding、remote search、Warp Drive | context/knowledge/codegraph | 补索引任务/HEAD snapshot/remote parity |
| memory | cloud memory store API | local memory/knowledge crates | 增加 provider-neutral memory contract |
| subagent | local/remote/cloud child、orchestration topology、ambient task | executor/worktree/DAG/store | 统一 parent-child run graph |
| schedule | server-backed scheduled ambient agents | scheduler crate | 先协议化 schedule，再接云端 |
| remote execution | SSH remote server、cloud environment、runner | SSH + Windows Sandbox/Sandboxie adapters | 补 remote worker capability/status protocol |
| Computer Use | screenshot/mouse/recording/mac support + dedicated model | vision service/cache，尚无同等完整 action loop | 先 capability/approval/replay，再实现 driver |
| artifact | PR/file/screenshot/external reference + harness support | tool artifact store + artifact events/collection | 补外部引用和上传状态协议 |
| usage/cost | credits/provider/category/cache/fallback | usage/cost stores + DeepSeek cache fields | 补 provider/category/fallback/credit semantics |
| retry/recovery | driver retry、credential refresh、idle linger | model attempt reset、stall detector、budget/context terminal | 合并 setup-level 和 provider-level retry taxonomy |
| security | secret redaction、managed secrets、team policy、isolation | secret scanner、permission、SandboxBackend、redaction | DeepAgent 更适合做统一 policy；补 workload token/audit |
| protocol | GraphQL/WebSocket/proto + JSON/NDJSON CLI | versioned stdio JSON-RPC + SDK | DeepAgent 的 harness 应继续作为统一入口 |
| replay | conversation restore/server task recovery | append-only run/session replay/fork/truncate | DeepAgent 基础更好；补富 item hydration |
| testing | extensive UI/TUI/integration tests | runtime/app-core/protocol tests | 为每个协议事件加 fixture/reconnect/terminal test |

## 7. DeepAgent Studio 的优势、劣势与风险

### 7.1 已领先的结构优势

- 单一执行真相：`AgentKernel` 负责 run lifecycle，RuntimeEngine 负责循环，ToolExecutionPipeline 负责工具门禁。
- 事件可 replay：持久化 append-only event 与 live projection 分离，支持 `afterSequence` 断线续读。
- 跨入口契约：CLI app-server、TypeScript SDK、Desktop 都能消费同一 harness protocol。
- 终态一致：成功、取消、阻塞、超步、预算、上下文耗尽、失败集中映射并写入一次。
- 安全 seam 清晰：权限、审批、hook、secret redaction、sandbox backend 都可测试替换。
- DeepSeek 适配保留 reasoning、cache usage、raw provider usage，不把供应商字段泄漏到 UI/CLI。

### 7.2 当前相对 Warp 的短板

- 缺少经过真实 PTY/terminal UX 验证的 agent action surface。
- 缺少成熟的 local/remote/cloud/ambient 任务运营与 joinable live session。
- 缺少 Warp 那样的 action-rich 富输出（diff、artifact、todo、citation、MCP resource、question、computer use）。
- action pipeline 尚未形成可持久化队列和多 action 原顺序语义。
- environment/skill/MCP setup 尚未成为 run 的一等可观测阶段和 snapshot。
- 第三方 CLI harness/session sharing/view-only/agent takeover 仍需产品化。

### 7.3 不能照搬 Warp 的风险

- 不要把 server conversation token、UI Entity、Tauri DTO、harness DTO 混为一个公共类型。
- 不要在 CLI/SDK/Desktop 各自新增 tool registry、approval、cancel、run store。
- 不要把 Warp 的 action 大枚举直接复制进 runtime；用稳定的 `ToolDescriptor` + capability + item payload，并对特殊富输出单独建类型。
- 不要把 cloud task 的状态假设成“本地进程状态”；远程 location、网络断线、租约、join/resume 必须显式建模。
- 不要把 view-only 当成“自动批准”；它应是权限/交互策略的独立 scope，并留下审计事件。

## 8. 建议的演进路线

### P0：统一动作与协议状态（立即）

1. 新增 `ToolInvocationState` 和 durable action sequence；状态覆盖 prepared、queued、blocked、running、completed、failed、cancelled。
2. `RuntimeEvent` 增加稳定 `item_id/call_id`、approval id、action sequence、parent run id；避免前端从 raw JSON 猜状态。
3. 将 hook、approval、checkpoint、provider retry、MCP startup/degraded、artifact progress 映射为一等 `HarnessEvent`。
4. 补 `thread/read` 断线重连、approval resume、terminal event exactly-once 的协议 fixtures。

### P1：环境、技能、MCP、成本平台化（短期）

1. 建立 `RunEnvironmentSnapshot`：cwd、workspace boundary、repo/ref/commit、sandbox backend/capabilities、network policy、skill source/hash、MCP server/version/status。
2. 为 provider profile 增加主模型/coding/vision/child model、fallback chain、credential source、cache expiry、usage/cost category。
3. 将 artifact store 扩展为 file/PR/screenshot/external-reference，协议提供 upload/start/completed/failed。
4. 增加 setup step 事件和耗时：terminal bootstrap、sandbox startup、MCP startup、skill load、indexing、model first token。

### P2：任务图、远程、终端交互（中期）

1. 用同一 RunStore 表示 root/child/background/remote run，增加 parent-child、execution location、join/resume、background、creator/executor。
2. 抽象 `ExecutionBackend`/`TerminalSessionBackend`，让 Direct、Sandboxie、Windows Sandbox、SSH、未来 remote worker 共享 command/PTY/approval contract。
3. 为 TUI/PTY 引入 input lease、takeover、long-running command、ask question、editable approval 的协议事件。
4. 将 `ComputerUse` 建模为 screen capture、pointer/keyboard action、approval、recording artifact、replay，而不是普通 JSON 工具。

### P3：云端运营与生态（后期）

1. schedule、ambient follow-up、cloud handoff、capacity/credits、team policy 接入 app-server protocol。
2. 提供 harness-support 类 side-channel，但通过统一 `run_id`/workload token/audit context，并由 AppCore 路由。
3. 支持第三方 CLI harness adapter；adapter 只能产生统一 runtime/harness events，不能绕过 ToolRegistry/ApprovalGate/SandboxBackend。

## 9. 推荐的目标状态机

```text
Thread (durable session)
  -> Turn accepted
  -> Run: accepted
  -> preparing
       terminal/sandbox/environment/skills/MCP/index
  -> running_turn
       model request -> reasoning/content/tool-call deltas
  -> action queue
       prepared -> queued -> blocked? -> running -> completed/failed/cancelled
       (safe actions may run concurrently; sequence preserves result order)
  -> waiting approval / waiting child / waiting remote session
  -> executing_tools / verifying
  -> finalizing
  -> terminal exactly once
```

推荐协议事件至少携带：`protocolVersion, threadId, runId, turnId, itemId, parentRunId, sequence, timestamp, executionLocation, provider/model, approvalId, capability, terminalKind`。展示文本、UI layout、TUI focus 等仍留在 UI DTO，机器协议只保留可重放的语义字段。

## 10. 最终判断

Warp 是目前本地资料中“agent + terminal + cloud operations + multi-agent + remote execution”结合最完整的参考实现；它最值得借鉴的是动作队列语义、富输出类型、环境准备顺序、终端 takeover、child/ambient task 状态和运营级 retry/telemetry。DeepAgent Studio 不应追求源码同构，而应把这些能力压缩成更少、更稳定的 runtime seam。

若以平台化目标衡量，推荐继续以 DeepAgent 的 `AgentKernel/RuntimeEngine/RunStore/EventStore/HarnessEvent` 为唯一骨架，按 P0-P3 逐步吸收 Warp 的产品能力。这样既能保留当前 DeepSeek、审批、MCP、沙箱、replay 和 SDK 的一致性，也能获得 Warp 在终端工作流、任务图、远程/后台运行和富交互上的成熟经验。

## 附录 A：可复核的关键源码定位

### Warp

- `借鉴/warp/AGENTS.md`：GUI/TUI 共享 core、开发命令和测试策略。
- `借鉴/warp/Cargo.toml`：workspace crate 与 GraphQL/WebSocket、多代理、Computer Use 依赖。
- `借鉴/warp/app/src/ai/agent/mod.rs`：取消结果、stream output、action/result、subagent、MCP/context、artifact 等类型。
- `借鉴/warp/app/src/ai/agent/api.rs`：`RequestParams`、conversation token、model/provider/autonomy/isolation/feature flags。
- `借鉴/warp/app/src/ai/agent/task.rs`：Task、parent task、exchange/message/context、working directory。
- `借鉴/warp/app/src/ai/blocklist/action_model.rs`：动作状态、队列、原序、预处理、执行和结果释放。
- `借鉴/warp/app/src/ai/agent_sdk/driver.rs`：AgentDriver、setup、harness、retry、错误分类、cleanup、输出。
- `借鉴/warp/app/src/ai/agent_sdk/driver/environment.rs`：多 repo clone、HEAD pin、environment snapshot、index。
- `借鉴/warp/app/src/ai/agent_sdk/harness_support.rs`：ping、artifact/reference、notify、finish、shutdown side-channel。
- `借鉴/warp/app/src/ai/ambient_agents/task.rs`、`spawn.rs`、`scheduled.rs`：后台任务、执行位置、live session、schedule。
- `借鉴/warp/app/src/ai/orchestration/providers.rs`：harness 的 model/host/environment/auth 选择持久化。
- `借鉴/warp/app/src/ai/mcp/`、`借鉴/warp/crates/isolation_platform/`、`借鉴/warp/crates/computer_use/`：MCP、隔离、Computer Use。

### DeepAgent Studio

- `crates/deepagent-runtime/src/kernel.rs`：RunPhase、TerminalKind、AgentKernel、持久化 sink、终态 exactly-once。
- `crates/deepagent-runtime/src/events.rs`：RuntimeEvent、工具/审批/subagent/checkpoint/usage/terminal 事件。
- `crates/deepagent-runtime/src/loop_engine.rs`：RuntimeEngine 的 think/stream/tool/verify/finalize 循环。
- `crates/deepagent-runtime/src/tool_pipeline.rs`：schema、hook、approval、执行、完成、checkpoint、artifact pipeline。
- `crates/deepagent-harness-protocol/src/requests.rs`：版本化 JSON-RPC request contract。
- `crates/deepagent-harness-protocol/src/events.rs`：HarnessEvent、ItemPayload、RuntimeEvent 投影与 replay context。
- `crates/deepagent-persistence/src/run_store.rs`、`event_store.rs`：run/session/event append、finish、read-after-sequence、fork/truncate。
- `crates/deepagent-app-core/src/chat_service.rs`：AppCore 装配、session run、MCP/skills/knowledge/verification/cost/sandbox seam。
- `crates/deepagent-app-core/src/sandbox_backend.rs`：Direct/Sandboxie/Windows Sandbox 后端与 `.wsb` task plan。
- `crates/deepagent-tools/src/permission.rs`、`registry.rs`、`sandbox.rs`：权限、风险、工具注册和 sandbox policy。
- `crates/deepagent-models/src/chat.rs`、`stream.rs`：thinking、tool schema、usage、SSE/Responses 流式累积。
- `apps/cli/src/app_server.rs`、`packages/sdk/src/index.ts`：CLI JSON-RPC server 与 TypeScript SDK 消费端。

## 附录 B：本次验证记录

- 已检查当前仓库 `git status --short --branch`：初始无本轮未提交改动，分支为 `main`（ahead 1）。
- 已读取双方 workspace manifest、仓库规则、关键源码和测试定位，并核对 Warp git commit。
- 已尝试官方网页检索；工具返回 `502 auth_not_found`，因此没有引用无法验证的网页宣传资料。
- 本轮只新增 Markdown 文档，不涉及 Rust/TypeScript 行为；交付前执行 Markdown 内容/链接路径检查、diff 检查和 Git 状态检查。未运行全 workspace 编译/测试，因为没有代码行为变更。
