# DeepAgent Studio 基于 Warp 证据的底层升级技术方案

> 文档性质：架构升级设计与实施基线，不是产品宣传稿。
>
> 调研对象：本仓库当前工作树；Warp 本地快照 `借鉴/warp`，commit `061318ff7fc424e41fbd77e30432995d483c99e4`。
>
> 目标：只吸收 Warp 中能够由本地源码复核、且确实对应 DeepAgent 底层不足的技术点。已经存在的能力不重复建设；无法由本地源码证明的 Warp 云端能力不作为事实或验收依据。

## 1. 先说结论

本系统目前最需要升级的不是再增加几个工具或再接一个入口，而是把已有能力从“进程内可运行”升级为“可恢复、可观测、可部署、可跨执行后端”。当前代码已经有不少正确的组件：`AgentKernel`、`RuntimeEngine`、`ToolExecutionPipeline`、`RunStore`、`EventStore`、审批 gate、输入租约、MCP registry、SandboxBackend、SSH、子代理 store 和 stdio harness。但这些组件之间还存在明显的控制面断层：

1. **运行状态持久化粒度不够**：`RunStore` 主要保存 run 宏观状态；工具调用、审批等待、setup 资源、执行租约没有统一的 durable control record。
2. **恢复语义不完整**：重启时子代理可以被取消或显式 resume，但主 run、pending approval、queued input、未完成工具和远程会话没有一套统一恢复策略。
3. **执行面还没有收敛**：Direct、Sandboxie、Windows Sandbox、SSH/remote、桌面 TerminalService 各自有能力描述，但 one-shot command、interactive PTY、artifact 和取消没有统一 session contract。
4. **协议已经可用但不够完整**：`thread/turn/item/approval` 已有基础协议；environment、capability snapshot、tool queue、artifact、MCP lifecycle、provider retry、child join 等关键机器语义仍被压进 generic runtime item 或根本没有协议字段。
5. **部署仍偏“桌面/单机进程”**：CLI stdio server、Tauri backend、SQLite 本地库是可用形态，但没有 worker/daemon、租约、队列、远程执行节点和健康检查的生产部署边界。
6. **观测能记录结果，不能完整解释准备过程**：Warp `AgentDriver` 的 terminal bootstrap、MCP resolve/start、skill/environment/preflight/harness/cleanup 是分阶段流程；本系统有 `RunEnvironment` 和 runtime log，但没有统一的 setup state machine 和 per-step duration/status contract。

因此建议的升级方向是：

```text
Harness / Desktop / CLI / SDK
          |
      Control API
          |
RunCoordinator + durable leases + action records
          |
AgentKernel -> RuntimeEngine -> ToolExecutionPipeline
          |
ExecutionBackend (Direct / Sandboxie / WindowsSandbox / SSH / future worker)
          |
SQLite local profile | PostgreSQL + object store + queue service profile
```

核心原则：不新增第二套 runtime、第二套 approval、第二套 event store；Warp 的能力必须投影到现有 seam。

## 2. 证据与现状分级

| 标签 | 含义 | 本文用法 |
|---|---|---|
| `S` | 当前源码直接证明 | 可作为当前实现事实 |
| `T` | 当前仓库测试直接证明 | 可作为行为事实，但要说明测试范围 |
| `I` | 由代码结构推断 | 只能作为设计判断，不能写成已实现 |
| `U` | 当前本地材料无法确认 | 不进入能力承诺或上线验收 |

### 2.1 DeepAgent 已有能力，不应重复建设

| 能力 | 证据 | 真实状态 | 升级要求 |
|---|---|---|---|
| 宏观 run 生命周期 | `crates/deepagent-runtime/src/kernel.rs` 的 `RunPhase`、`TerminalKind`、`PersistentEventSink` | `S/T` 已有 accepted 到 terminal、终态分类、持久化 sink | 细化 setup/action 状态，不另造 kernel |
| 工具门禁 | `crates/deepagent-runtime/src/tool_pipeline.rs`、`loop_engine.rs` | `S/T` 已有 schema、hook、permission、approval、checkpoint、artifact preparation | 保持门禁顺序，增加 durable record |
| 并行工具 | `loop_engine.rs:1148-1378`；并行测试 | `S/T` 安全工具和隔离 `task` 可并发，live 按完成顺序，session log 按调用顺序 | 不重写并发；补队列和恢复 |
| 事件 replay | `deepagent-persistence/src/event_store.rs`、`run_store.rs`、harness `thread/read` | `S/T` 有 session event、run event、after sequence、fork/truncate | 统一 cursor 范围和富 item hydration |
| 审批 | `deepagent-app-core/src/approval_bridge.rs` | `S/T` 内存 pending registry + async gate + UI/CLI callback | 持久化等待、过期和作用域 |
| 输入租约 | `deepagent-runtime/src/input.rs`、`app-core/src/input_runtime.rs` | `S` 有进程内 session lease、queued input、取消 active run | 将 lease owner/expiry/recovery 写入控制面 |
| 子代理 | `deepagent-subagents`、`persistence/subagent_store.rs` | `S/T` 有 DAG scheduler、parent id、worktree、resume、orphan cancel | 与主 run 统一 graph 和事件查询，不复制 executor |
| MCP | `deepagent-mcp` registry/transport/reconnect/liveness | `S/T` 有 stdio/http/SSE、重连、活性检查 | 增加 run snapshot、启动阶段和 degradation 事件 |
| 沙箱 | `app-core/src/sandbox_backend.rs` | `S/T` Direct/Sandboxie/WindowsSandbox、能力描述、workspace 边界、`.wsb` | 统一 PTY/one-shot/artifact/capability contract |
| SSH | `deepagent-ssh`、Tauri SSH commands | `S/T` 有探测、文件/包传输、远程命令与 PTY commands | 接入 ExecutionBackend，不另做 remote tool 链 |

### 2.2 不能写成系统优势的地方

- `RunStore::finish` 的 exactly-once 只约束通过该方法的数据库终态更新，不等于所有外部资源都 exactly-once。
- `PersistentEventSink` 说明 run event log 是当前 harness projection 的事实源，不等于项目中只有一类事件存储；Session EventStore、RunStore、runtime logs、artifact store 各自有边界。
- `SubagentRunStore` 的 resume/orphan cancel 不是完整的 remote worker 调度器；它没有解决 worker 租约、网络断线、重投递和跨主机身份。
- `WindowsSandboxBackend` 支持 one-shot 和受限映射，但 `supports_interactive_pty` 明确为 `false`；不能宣称已经完成 Warp 式 interactive terminal。
- CI 已覆盖 Rust 多平台和桌面前端构建，但没有生产 daemon、远程 worker、跨进程 crash recovery 或真实云服务验收。

## 3. Warp 可借鉴点与本系统真实缺口

### 3.1 ActionModel：从内存工具调用升级为 durable action control plane

#### Warp 可借鉴的源码事实

Warp `app/src/ai/blocklist/action_model.rs` 明确维护 `Preprocessing`、`Queued`、`Blocked`、`RunningAsync`、`Finished(result)` 五态，并为每个 conversation 保存 `VecDeque`、running action、finished result 和原始 `action_order`。`action_model_tests.rs` 直接验证结果按原始 action 顺序排序。这个模型解决的是：一批 tool call 同时到达时，哪些可以并发、哪些必须等待用户、结果如何稳定回填。

#### DeepAgent 的真实缺口

DeepAgent 的 `loop_engine` 已经解决执行并发，但 `RunStore`/migrations 没有以 `run_id + call_id` 为核心的 action record、状态迁移、attempt、lease、blocked reason、resume token。`ToolBlocked` 是事件，不是可查询的 durable queue item。审批 pending 也由 `HashMap<String, oneshot::Sender>` 保存，进程重启后无法仅凭数据库恢复等待。

#### 目标设计

新增 `run_actions` 表和 `ToolInvocationState`，但它必须是 Runtime/RunStore 的一部分，不新增独立 action store：

```text
received -> prepared -> queued -> blocked -> running -> completed
                                      |          |
                                      |          +-> failed / cancelled
                                      +-> expired / denied
```

最少字段：`run_id`、`turn_id`、`call_id`、`sequence`、`tool_name`、`arguments_hash`、`state`、`risk`、`approval_id`、`attempt`、`lease_owner`、`lease_expires_at`、`started_at`、`finished_at`、`result_ref`、`blocked_reason`、`parent_action_id`。

状态迁移必须通过单一 service，禁止 UI、CLI、SDK 直接更新 SQL。事件仍写 `run_events`，action table 是可查询 projection；两者在同一 SQLite transaction 中写入，生产数据库使用事务和唯一约束保证幂等。

#### 不应照搬 Warp 的地方

- 不把 31 个 `AIAgentActionType` variant 复制为 DeepAgent runtime 大 enum；保留 `ToolDescriptor + typed ItemPayload`，特殊富输出再按领域建类型。
- 不把 action queue 放进 UI model；队列必须由 runtime control plane 管理。
- 不允许“blocked = auto approve”。blocked 需要 approval scope、policy snapshot 和审计上下文。

#### 验收

1. 模型一次返回 8 个工具调用，4 个安全读并发，2 个审批，2 个依赖前置结果；重启进程后能查询每个 action 的准确状态。
2. approval/respond 重复发送、乱序发送、过期发送都幂等且有明确错误码。
3. action result 按模型调用 sequence 回填，live event 可以按完成顺序发出。
4. crash 注入在 prepared/blocked/running 三个点，恢复后不会重复副作用。

### 3.2 AgentDriver setup：把 Preparing 变成可观测、可重试的 setup state machine

#### Warp 可借鉴的源码事实

Warp `AgentDriver::run_internal` 把 cwd 检查、terminal bootstrap、cloud provider setup、MCP resolve/start、skills resolve、environment clone/HEAD pin、preflight、harness/plugin setup、run、retry/linger/cleanup 分成不同阶段。失败不是一个无上下文的 `run failed`，而是能知道失败发生在 setup 哪一步。

#### DeepAgent 的真实缺口

DeepAgent `RunPhase::Preparing` 是单一宏观阶段。`RunEnvironment::resolve` 能解析配置、权限、sandbox mode 并写 runtime log；ChatService 也装配 skills/MCP/verification，但这些步骤没有统一的 `SetupStep` 身份、状态、重试策略和完成事件。发生 MCP 启动慢、技能来源失效、sandbox 启动失败时，协议消费端无法稳定区分。

#### 目标设计

在 `deepagent-app-core` 增加 `RunSetupCoordinator`，只负责编排，不执行模型循环：

```text
validate_workspace
 -> resolve_config_and_policy
 -> probe_execution_backend
 -> resolve_skills
 -> resolve_mcp
 -> start_mcp
 -> capture_environment_snapshot
 -> preflight_tools_and_hooks
 -> create_runtime
```

每个 step 输出 `setup.started / setup.updated / setup.completed / setup.failed`，包含 `step_id`、`attempt`、`duration_ms`、`retryable`、`degradation_code`、`redacted_detail`。`Preparing` 仍保留为 RunPhase，setup step 是其 item 级投影。

#### 环境快照

新增 `RunEnvironmentSnapshot`，记录：canonical cwd、workspace root、git repo/ref/HEAD、sandbox backend/capabilities、network policy、permission profile hash、loaded config source、skill id/version/hash/scope、MCP server id/version/status、provider/model profile。凭据只记录来源和 fingerprint，不记录 secret。

#### 验收

- MCP server 启动失败但 run 可降级时，客户端收到明确 `degradation_code`，而不是“工具不存在”。
- 相同 workspace/config 生成稳定 snapshot hash；重启恢复能判断环境是否已变化。
- 每个 setup step 有开始/结束时间、attempt 和可重试分类。

### 3.3 Harness capability snapshot：从静态 `tool/list` 升级为 run-scoped negotiation

#### Warp 可借鉴的源码事实

Warp `get_supported_tools` 根据 execution mode、session type、feature flag、Computer Use 开关和入口动态生成 `supported_tools`。这意味着模型可见的能力是一次 run 的协商结果，而不是全局注册表。

#### DeepAgent 的真实缺口

DeepAgent `tool/list` 调用 `chat.tool_descriptors()`，可以列工具，但协议没有稳定的 run-scoped snapshot、schema hash、风险、并发安全、sandbox capability、来源和版本。一个工具可能在全局 registry 中存在，但当前 backend 不支持，模型仍可能生成调用。

#### 目标设计

`tool/list` 保持兼容；新增 `capabilities/resolve` 或在 `turn/start` response 返回：

```json
{
  "snapshotId": "cap_...",
  "tools": [{
    "name": "bash",
    "schemaHash": "sha256:...",
    "risk": "high",
    "concurrency": "exclusive",
    "requiresApproval": true,
    "requires": ["command", "workspace_write"],
    "source": "builtin",
    "resolvedBy": "runtime"
  }],
  "backend": {"kind": "windows_sandbox", "pty": false, "network": "disabled"}
}
```

snapshot 必须落盘并与 run 关联，replay 使用历史 snapshot，不用当前 registry 重新解释历史。

### 3.4 富 action/result：补机器语义，不复制 UI

Warp action/result 的价值在于 result typed、取消可回填、文件 diff、PTY write/read、question、artifact、MCP resource、child message 都有明确语义。DeepAgent 当前 `HarnessEvent::ItemPayload` 主要覆盖 content/reasoning/tool call/result/usage/subagent，其他 runtime event 会落成 generic `Runtime`。

真实不足不是“没有工具”，而是以下语义还不能被所有客户端稳定还原：

| 语义 | 当前状态 | 升级 |
|---|---|---|
| 文件 diff/可编辑批准 | 有 file tools、checkpoint、diff service，但协议没有独立 diff item 合同 | `file.patch.proposed/approved/applied/rejected`，带 patch hash 和 workspace boundary |
| artifact 上传 | 有 artifact store/collection，但上传进度与外部引用不统一 | `artifact.created/uploading/completed/failed`，内容使用 ref，不把大 blob 放事件 |
| ask user | 有 ask-user builtin，但 harness 没有一等 question request/answer | `input.requested/responded`，区分 approval 与普通问题 |
| MCP resource | 有 registry/adapter | 增加 server instance、resource URI、degradation、result ref |
| PTY write/read | SSH 和桌面有 PTY 命令，但 runtime tool contract 未统一 | 见 3.5 input lease/terminal session |
| cancellation result | Kernel 有 terminal cancel，工具结果有 failure observation | 为 action 增加 `cancelled_by`、`continuation`、`resume_token` |

#### 当前协议实现的两个具体风险

1. `apps/cli/src/app_server.rs` 的 `thread_read` 同时读取 Session `EventStore` 和多个 run 的 `RunStore`，却把同一个 `afterSequence` 传给两种不同的 sequence。它们分别是 session-local 与 run-local 序列，不能被客户端当作一条全局 cursor。升级时必须返回带 `stream`/`runId` 的 cursor，或在 projection 层生成真正的 thread-global sequence；不能只改字段名。
2. `run_stdio` 的 event emitter 对每个事件调用 `tokio::spawn` 后独立获取 stdout lock。lock 能避免字节交错，但不能保证 spawn 顺序就是写出顺序。对于需要 replay/ack 的客户端，应改为每个连接一个有界发送队列和单 writer task，队列满时产生 backpressure/drop policy 事件。

这两点说明“有 JSON-RPC 和 afterSequence”不等于已经具备严格的跨流顺序和断线续读语义。

### 3.5 Terminal/PTY/input lease：Warp 终端能力的底层引入方式

#### 真实缺口

当前 `SandboxCapabilities` 已声明 interactive PTY 能力，Direct backend 为 `true`，Sandboxie/Windows Sandbox 为 `false`；但 Runtime 的 command tool、SSH PTY、桌面 TerminalService 尚未共享统一的 terminal session id、输入租约、输出 cursor、takeover 和恢复语义。`InputLeaseRegistry` 只解决 session dispatch lease，不是 PTY input ownership。

#### 目标设计

新增 `TerminalSessionBackend` trait，作为 `ExecutionBackend` 的子能力：

```text
open -> ready -> running
                 |  \
                 |   -> exited
                 -> takeover_requested -> user_owned -> released/resumed
```

接口至少包含 `open`、`write`、`read(after_cursor)`、`resize`、`signal`、`takeover`、`release`、`close`。每个 session 有 `input_lease`，租约由 runtime、user、remote viewer 之一持有；所有写入都验证 lease 和 run scope。

Direct/SSH 先实现；Sandboxie/Windows Sandbox 如果 backend capability 为 false，协议必须返回 `unsupported_capability`，不能 fallback 到 host 直执行。

### 3.6 Cancellation 与 continuation：从“取消 run”升级为业务控制信号

Warp 区分 cancellation reason 与 cancellation outcome，例如 CLI takeover、follow-up、shell user execution、cloud handoff 并不都代表 conversation 失败。DeepAgent 目前 `CancellationTree`/`RunOutcome::Cancelled` 能可靠停止运行，但 `turn/steer` 通过取消旧 turn 再启动新 turn，缺少统一的 continuation disposition。

新增：

- `InterruptReason`：user、steer、takeover、timeout、parent_cancelled、backend_lost、shutdown。
- `ContinuationDisposition`：terminal_cancelled、resume_same_run、new_turn、external_finalize、keep_child_running。
- `cancel.requested`、`cancel.propagated`、`cancel.observed`、`continuation.created` 事件。

Kernel 仍是唯一写 terminal 的地方。这个设计借鉴 Warp 的业务语义，但保留 DeepAgent 的 exactly-once 终态边界。

### 3.7 Child/background/remote run graph：合并现有两套持久化投影

#### 真实现状

主 run 在 `runs/run_events`，子代理在 `subagent_runs`；后者已有 `parent_run_id`、`origin_parent_run_id`、worktree、resume_count 和 orphan cancel。这个基础可用，但协议的 `Subagent` item 与 RunStore 的 run 查询还不是同一个 graph API。

#### 目标设计

不立即删除 `subagent_runs`，先建立统一 `RunGraphView`：

```text
root run
 ├─ foreground child
 ├─ background child
 └─ remote child
      ├─ execution_location
      ├─ worker_id / lease
      ├─ join_token
      └─ resume_policy
```

新增 graph projection 字段：`parent_run_id`、`origin_run_id`、`execution_location`、`worker_id`、`lease_id`、`join_token_hash`、`background`、`resume_policy`。主 run 和 child event 查询按同一 `after_sequence`/per-run cursor 返回，避免客户端分别调用两套接口。

### 3.8 MCP/skills/environment 生命周期：从“可装配”升级为“可审计依赖”

Warp driver 的 MCP/skills/environment 解析和启动有明确阶段。DeepAgent 已有 MCP reconnect/liveness、skills registry/marketplace、workspace snapshot/codegraph，但 run 事件没有稳定记录“哪个版本、哪个 hash、何时解析、是否降级”。

升级内容：

1. `SkillResolution`：id、source、version、content_hash、scope、loaded_at、status。
2. `McpResolution`：server_id、config_hash、transport、tool_schema_hash、startup_attempt、status、degradation_code。
3. `WorkspaceSnapshot`：repo root、HEAD、dirty state、scanner/index version、snapshot hash。
4. 所有 snapshot 只写 metadata；大文本、工具 schema 和 artifact 放对象/文件引用。

### 3.9 Provider/retry/usage：保留 DeepSeek 特性并补运营维度

当前 `deepagent-models` 已有 SSE/Responses accumulator、reasoning、usage、raw provider usage、cache hit/miss 和 attempt reset。真实不足是 setup/provider attempt 的统一统计，以及主模型、coding、vision、child model、fallback chain 的 profile 化表达。

目标 `ModelProfile`：

```text
primary -> coding -> vision -> child
             \-> fallback providers
```

每次 attempt 写 `provider_attempt.started/completed/failed`，包含 provider、model、reason、retryable、backoff_ms、ttft_ms、usage_ref。credential 只记录 secret source/fingerprint。不要把 Warp credits 或未知服务端计费字段当成本系统事实；本系统先做 provider-neutral cost contract。

### 3.10 Persistence：SQLite 单机 profile 与 server profile 分层

当前 persistence 以 SQLite 为中心，`RunStore` 使用 `BEGIN IMMEDIATE` 分配 gapless run sequence，`EventStore` 是 append-only session ledger；这适合桌面和单进程。直接把 SQLite 当多 worker 共享控制库是不合理的。

目标分两档：

| Profile | 使用场景 | 存储与执行 |
|---|---|---|
| `desktop-local` | Tauri/CLI 单机 | SQLite、文件 artifact、单进程 coordinator、Direct/Sandboxie/WindowsSandbox |
| `server-worker` | 长任务/多客户端 | PostgreSQL（run/action/event projection）、对象存储（artifact/blob）、队列（dispatch）、worker lease、SSH/remote backend |

迁移原则：保留 domain DTO 和 repository trait；先让 SQLite 实现通过同一 trait，再增加 PostgreSQL 实现。不要让 runtime 直接依赖 rusqlite SQL。

## 4. 目标分层架构

### 4.1 Control Plane

新增 `RunCoordinator`（建议位于 `deepagent-app-core`，协议 DTO 位于 `deepagent-harness-protocol`）：

- 接受 start/resume/interrupt/steer/approval/action response。
- 为 run、turn、action、approval、terminal session 分配 id。
- 维护 durable lease、幂等键和状态迁移。
- 将命令投递给 RuntimeEngine 或 ExecutionBackend。
- 从 projection 生成 harness event，不让 UI 直接读内部表。

### 4.2 Runtime Plane

保持现有：`AgentKernel -> RuntimeEngine -> ToolExecutionPipeline`。只增加：

- action state persistence adapter；
- setup coordinator callback；
- interruption/continuation context；
- capability snapshot；
- provider attempt hooks。

### 4.3 Execution Plane

定义：

```rust
trait ExecutionBackend {
    fn capabilities(&self) -> BackendCapabilities;
    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult>;
    async fn open_terminal(&self, request: TerminalRequest)
        -> Result<Option<TerminalSession>>;
}
```

实际代码中可先通过现有 `SandboxBackend`、`CommandExecutor`、SSH service 适配，不要求一次重构所有工具。

### 4.4 Data Plane

建议新增/扩展表：

| 表 | 用途 | 关键约束 |
|---|---|---|
| `runs` | run projection | `finished_at` exactly-once；新增 location/parent/lease |
| `run_events` | ordered run ledger | `(run_id, sequence)` 唯一；payload redaction |
| `run_actions` | action projection | `(run_id, call_id)` 唯一；状态迁移受限 |
| `run_approvals` | approval projection | approval scope、expiry、decision、decided_by |
| `run_setup_steps` | setup projection | `(run_id, step_id, attempt)` 唯一 |
| `execution_leases` | worker/PTY/input lease | owner、epoch、expires_at、revoked_at |
| `run_capabilities` | capability snapshot | snapshot hash、tool schema hash、backend caps |
| `run_artifacts` | artifact metadata | content hash、size、state、external ref |
| `subagent_runs` | compatibility projection | 逐步投影到 RunGraphView |

## 5. 部署架构升级

### 5.1 当前部署问题

当前 CLI `server` 只支持 stdio；Tauri backend 直接在桌面进程中装配 ChatService；默认数据库是 workspace 下 `.deepagent/deepagent.db`；CI 验证的是构建和单机测试。这样的形态适合开发和个人桌面，但不适合：长时间后台任务、多个客户端同时观察、进程升级不中断、远程 worker、跨机器 artifact、统一审计。

### 5.2 三档部署形态

#### A. Desktop Local（近期默认）

```text
Tauri UI / CLI
      |
  in-process AppCore
      |
 SQLite + local artifact directory
      |
 Direct / Sandboxie / Windows Sandbox
```

优化：

- 加一个 single-instance `RunCoordinator`，避免多个 Tauri command 各自启动独立 ChatService。
- 数据库放到应用数据目录，workspace 只保留可选 project metadata；支持 migration lock 和备份。
- 启动时扫描 unfinished runs：按 lease 过期策略标记 interrupted/recoverable，不直接把所有 run 当成功或静默丢弃。
- artifact 使用 content hash、临时文件、fsync、rename，避免半写文件被 replay。

#### B. Local Daemon（中期）

```text
Tauri / CLI / SDK --stdio or local HTTP
              |
        deepagentd
              |
      Coordinator + Runtime
              |
      SQLite + local workers
```

优化：

- Tauri 不再直接持有长运行 loop，只消费 protocol event。
- daemon 负责单一 run/approval/action registry；客户端断开不终止 run。
- 首先实现 stdio JSON-RPC 和本机 named pipe/loopback HTTP 之一，协议稳定后再开放远程 HTTP/WebSocket。
- 引入 `/health/live`、`/health/ready`、`/metrics`，但不把内部 SQLite schema 暴露为公共 API。

#### C. Server + Worker（后期）

```text
Clients -> App Server -> PostgreSQL / Queue / Object Store
                           |
                    worker lease scheduler
                       /       |       \
                 local       SSH      sandbox worker
```

必须先具备：

- PostgreSQL repository 实现和 migration；
- action/approval/worker lease 的 CAS 更新；
- at-least-once dispatch + action idempotency；
- worker heartbeat、lease expiry、fencing token；
- artifact object store multipart/upload state；
- tenant/workspace permission boundary；
- remote worker 只接收 resolved capability 和最小凭据，不接收完整 host secret。

不建议现在直接上 Kubernetes。当前底层还没有 worker lease、幂等 action 和远程事件 cursor，先把这些契约在本机 daemon 跑通，再做容器编排。

### 5.3 部署安全边界

- Direct backend 必须显式显示为 host execution，并在 capability 中标记 `sandbox=false`。
- `FullAccess`、host workspace writable mapping、network enabled 必须带 approval scope 和审计事件。
- Windows Sandbox 映射 workspace、staging、output 目录必须最小化；禁止用全局临时目录作为长期 artifact 根。
- worker lease 失效时只允许重试无副作用或具备 idempotency key 的 action；命令执行不能默认重放。
- 日志、event payload、provider raw usage、artifact metadata 统一经过脱敏；secret 只在 dispatch 时解析。

## 6. 分阶段实施路线

### P0：控制面和恢复基线（必须先做）

1. `run_actions`、`run_approvals`、`execution_leases` migration。
2. `RunCoordinator` 统一 start/resume/interrupt/approval/respond。
3. action 状态迁移 service 和 idempotency key。
4. setup step event 与 environment snapshot metadata。
5. crash recovery fixtures：prepared、blocked、running、terminal。

**P0 完成标准**：进程被杀后，客户端通过 `thread/read` 能得到 run/action/approval 的一致状态；不会重复写 terminal，不会自动重放高风险副作用。

### P1：协议和执行后端收敛

1. capability snapshot 和 tool schema hash。
2. typed artifact、file diff、question、MCP lifecycle 事件。
3. `ExecutionBackend` 适配 Direct/Sandboxie/WindowsSandbox/SSH。
4. `TerminalSessionBackend` 先实现 Direct + SSH 的 read/write/resize/signal/lease。
5. continuation disposition 和 takeover 事件。

**P1 完成标准**：CLI、SDK、Desktop 对同一 run 的 action、approval、artifact、terminal session 看到相同机器语义。

### P2：后台任务和 daemon

1. Local Daemon 单实例 coordinator。
2. RunGraphView 合并主 run 与子代理查询。
3. daemon restart、client reconnect、approval expiry、input queue 恢复。
4. health/readiness/metrics 和 setup latency dashboard。

**P2 完成标准**：客户端退出不影响后台 run；daemon 重启后可恢复可恢复状态；不可恢复状态有明确 terminal reason。

### P3：Server + Worker

1. PostgreSQL/Object Store/Queue profile。
2. worker lease/fencing/idempotency。
3. SSH/Windows remote worker capability negotiation。
4. multi-tenant policy、audit export、quota/cost aggregation。

**P3 完成标准**：网络断开、worker 崩溃、重复 dispatch、artifact 重试和权限撤销都有可测试结果。

## 7. 关键测试与可观测性要求

### 7.1 必须新增的测试层

| 层级 | 测试 |
|---|---|
| Unit | action state transition、approval expiry、lease fencing、capability hash、snapshot hash |
| Persistence | transaction rollback、unique idempotency、sequence gap、projection rebuild、migration downgrade refusal |
| Runtime | parallel action + blocked approval + cancellation + continuation |
| Protocol | request/response/event golden fixture、unknown event forward compatibility、reconnect after sequence |
| Protocol ordering | session stream 与 run stream 使用独立 cursor；单 writer 在高并发事件下保持顺序；断线后无重复/漏事件 |
| Recovery | kill process at every setup/action state and reopen database |
| Backend | Direct/SSH contract fixture；Sandbox capability mismatch；Windows `.wsb` path boundary |
| E2E | CLI/SDK/Desktop 观察同一 run；多客户端 approval；artifact upload retry |
| Security | secret redaction、permission escalation、host path traversal、stale lease write |

### 7.2 必须观测的指标

- `run_setup_duration_ms{step}`
- `run_time_to_first_token_ms`
- `run_action_queue_depth{state,tool}`
- `run_approval_wait_ms`、approval expiry count
- `run_backend_start_ms{backend}`、PTY takeover count
- `run_provider_attempts{provider,model,outcome}`
- `run_reconnect_count`、event cursor lag
- `run_artifact_upload_bytes`、retry/failure
- `run_child_active`、child completion latency
- `run_terminal_duplicate_attempts`

指标只能包含脱敏标签，不能把 prompt、命令全文、token、workspace 私密路径作为 label。

## 8. 明确不做的事情

1. 不复制 Warp 的 `AIAgentActionType` 大枚举到 DeepAgent runtime。
2. 不在 CLI、Desktop、SDK 各自实现 approval、cancel、queue 或 event persistence。
3. 不把 SQLite 直接改成多进程/多主共享数据库来冒充 server control plane。
4. 不在没有 action idempotency 和 worker fencing 前上线远程自动重试。
5. 不把 Computer Use 当作普通 JSON tool；先完成 screen/input/approval/recording/replay contract。
6. 不把 Warp 的闭源云端能力、credits、SLA 或线上成熟度写进本系统验收标准。
7. 不因为已有 `SSH`、`WindowsSandbox` 或 `SubagentRunStore` 就宣称 remote worker、interactive terminal 或 durable orchestration 已完成。

## 9. 最终技术判断

Warp 对本系统最有价值的不是“功能更多”，而是它暴露了几个底层事实：多 action 到达后需要显式状态机；准备环境需要分阶段可观测；模型能力需要按 run 协商；终端控制权需要 lease；取消需要区分控制信号和最终业务结果；后台/远程任务需要 location、join、resume 和 worker 状态。

DeepAgent 当前已有 runtime、审批、事件、sandbox、MCP、SSH、subagent 的零件，但还没有完全把这些零件组织成 durable control plane。因此升级优先级必须是：

```text
P0 durable control plane
 -> P1 protocol/capability/execution convergence
 -> P2 local daemon/background recovery
 -> P3 server-worker deployment
```

在 P0/P1 没有完成前，继续增加第三方 harness、云端调度或 Computer Use 功能，会放大恢复、权限和部署风险。这个判断不是夸大本系统不足，而是由当前内存 approval、宏观 RunStore、分离的 subagent projection、PTY 能力不统一以及 stdio/desktop 单机部署事实直接推导出来的。

## 附录：主要证据路径

### DeepAgent Studio

- `crates/deepagent-runtime/src/kernel.rs`
- `crates/deepagent-runtime/src/loop_engine.rs`
- `crates/deepagent-runtime/src/tool_pipeline.rs`
- `crates/deepagent-runtime/src/events.rs`
- `crates/deepagent-runtime/src/input.rs`
- `crates/deepagent-persistence/src/run_store.rs`
- `crates/deepagent-persistence/src/event_store.rs`
- `crates/deepagent-persistence/src/migrations.rs`
- `crates/deepagent-persistence/src/subagent_store.rs`
- `crates/deepagent-app-core/src/approval_bridge.rs`
- `crates/deepagent-app-core/src/input_runtime.rs`
- `crates/deepagent-app-core/src/run_environment.rs`
- `crates/deepagent-app-core/src/sandbox_backend.rs`
- `crates/deepagent-app-core/src/chat_service.rs`
- `crates/deepagent-mcp/src/registry.rs`
- `crates/deepagent-mcp/src/reconnect.rs`
- `crates/deepagent-ssh/src/service.rs`
- `crates/deepagent-subagents/src/scheduler.rs`
- `crates/deepagent-harness-protocol/src/requests.rs`
- `crates/deepagent-harness-protocol/src/events.rs`
- `apps/cli/src/app_server.rs`
- `apps/cli/src/main.rs`
- `apps/desktop/src-tauri/tauri.conf.json`
- `.github/workflows/ci.yml`

### Warp

- `借鉴/warp/app/src/ai/blocklist/action_model.rs`
- `借鉴/warp/app/src/ai/blocklist/action_model_tests.rs`
- `借鉴/warp/app/src/ai/agent/api/impl.rs`
- `借鉴/warp/app/src/ai/agent_sdk/driver.rs`
- `借鉴/warp/app/src/ai/agent_sdk/driver/harness/mod.rs`
- `借鉴/warp/app/src/ai/agent_sdk/driver/environment.rs`
- `借鉴/warp/app/src/ai/agent_events/driver.rs`
- `借鉴/warp/app/src/ai/agent_events/message_hydrator.rs`
- `借鉴/warp/app/src/persistence/agent.rs`
- `借鉴/warp/app/src/ai/restored_conversations.rs`
- `借鉴/warp/crates/ai/src/agent/action/mod.rs`
- `借鉴/warp/crates/ai/src/agent/action_result/mod.rs`
- `借鉴/warp/crates/jsonrpc/`

## 附录 B：本轮验证

- 已完成当前工作树与 Warp 快照的源码路径复核。
- 已执行 `cargo test -p deepagent-persistence run_terminal_is_exactly_once_and_events_are_gapless --offline`：通过。
- 已执行 `cargo test -p deepagent-runtime parallel_tools_run_and_feed_back_all_observations --offline`：通过。
- 本文只新增技术文档，不修改 Rust/TypeScript 运行行为；未声称完成 P0-P3 实施。
