# DeepAgent Studio 全系统 Harness 审查

## 审查口径

- 开始日期：2026-10-21；基线提交：`e70504e`。
- 目标：逐项核查 Harness、DeepSeek 适配、上下文、运行与取消、工具与审批、MCP/插件、权限与沙箱、持久化恢复和各宿主入口；与仓库中的成熟 Agent 实现对照。
- 本文随验证推进更新。代码疑点、失败的测试断言和已复现的产品缺陷分别记录，不能互相替代。
- 每项问题需具备触发条件、生产路径、可重跑验证、参考实现、影响及修复方向。尚未覆盖的模块不视为通过。
- 测试密钥、完整请求、用户历史和机器私有配置不进入文档。运行数据位于忽略提交的 `target/`。

## 覆盖进度

| 范围 | 当前证据 | 审查状态 |
|---|---|---|
| Windows Tauri → AppCore → DeepSeek | 当前源码编译、真实连接、连续对话、取消和数据库终态 | 已获得基线，仍需故障注入与边界验证 |
| stdio Harness / SDK 事件交付与恢复 | 队列丢终态与立即中断窗口均已确定性复现 | 已确认 H-01、H-02；其余路径未完成 |
| DeepSeek 请求、reasoning、工具协议、usage | 真实请求和 usage 返回；尚未覆盖全部错误和流式分支 | 进行中 |
| 上下文、压缩、记忆 | 连续对话与笔记用例；笔记请求数量断言失败 | 进行中，未据此认定业务故障 |
| 工具、审批、权限与沙箱 | 真实文件读取及工作区外拦截；沙箱 backend override 路由探针 | 已确认 H-03；其他边界未完成 |
| MCP、插件与 hooks 生命周期 | 待逐项验证 | 未完成 |
| 持久化、崩溃恢复、并发与输入租约 | 取消终态已读取，其他边界待验证 | 未完成 |
| CLI / SDK / 桌面 / 工作流与子 Agent 一致性 | 待逐项验证 | 未完成 |

## 实测基线：Windows 桌面与真实 DeepSeek

### 运行方法

1. `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --offline`：通过。
2. `apps/desktop` 中运行 `pnpm dev`；启动上述构建的 `deepagent-desktop.exe`，使用独立 `DEEPAGENT_DATA_ROOT=target/live-desktop-test-20261021`。
3. 真实 WebView2 中完成连接引导；发现并选择 `deepseek-flash`。在该桌面窗口输入、发送和停止；通过本机 CDP 观察 IPC，并读取测试数据库 `runs` / `run_events`。
4. 独立运行 `cargo test -p deepagent-app-core --features keychain,web --test real_session_file_responses real_responses_session_replays_tools_skills_files_and_logs -- --ignored --nocapture`，密钥通过进程环境注入。

普通浏览器中的 `api.ts` 会回退模拟数据（参见 `getInvoke()` / `mockChatStream()`），因此浏览器预览成功不能证明真实后端通过。此次测试连接的是实际 Tauri WebView。

### 已确认结果

| 场景 | 结果及证据范围 |
|---|---|
| 新会话简短回复 | 正确回复“测试成功”；provider 输入约 16.8k、输出 4；数据库从接纳到模型请求 804ms、接纳到终态 2170ms |
| 同会话继续 | 正确复述上一轮内容；接纳到模型请求 497ms、到终态 1637ms；证明此输入的历史继续链路可用 |
| 其余已发起模型请求的桌面运行 | 6 次接纳到模型请求分别为 804 / 497 / 568 / 625 / 507 / 615ms；样本不足以声称达到 P95 性能目标 |
| 输出中停止 | 一轮发送后约 2 秒点击停止；终态 `cancelled`，事件有 `cancel.observed`、`run_cancelled`、`cancel.requested`、`cancel.propagated` 和单个 `run_terminal` |
| 接纳后立即停止 | 记录到 `start_chat_v2` 响应后点击；`cancel_run` 请求至 HTTP 响应约 4ms；数据库运行总时长 210ms，终态 `cancelled`，`model_request_started` 数为 0 |
| 模拟挂起模型的取消回归 | `cargo test -p deepagent-app-core --test golden_trace cancel_request_acknowledged_within_200ms -- --nocapture`：1 passed |
| 真实只读工具调用 | 工作区内放入未在提示中透露内容的随机标记文件；模型调用 `read_file` 后回复与文件一致。文件放在工作区外时被权限钩子拦截；未修改权限绕过 |
| 桌面进程重启与继续对话 | 正常关闭并重启同一隔离数据目录的桌面进程；展开折叠的“最近对话”后打开原会话，历史与标记仍可见；发起新一轮要求复述标记且不调用工具，模型正确复述 |
| 真实三轮 AppCore 用例 | 完成三轮；每轮持久化 provider usage 分别为 12468 / 13101 / 14019 输入 tokens，cache hit + miss 等于输入；后续测试断言失败，整条用例不算通过 |

### 尚未定性的现象

1. **笔记请求次数断言失败**：真实用例在 `real_session_file_responses.rs` 的 `note_requests % 3 == 0` 处失败，观察到 6 次主 Agent 请求 + 4 次笔记请求。生产笔记代理允许正常工具循环，并未要求每轮恰好 3 次请求。需要核对笔记内容、游标及实际执行结果，不能把此计数直接认定为生产故障。
2. **第一次与第二次上下文总量下降**：桌面两轮投影分别约 16.8k → 15.5k。分项中动态环境约 3723 → 1969 tokens，而对话分项 26 → 41 tokens；不能据总量下降认定历史丢失。需要继续验证动态环境变化来源和 wire history。
3. **立即点击停止的自动化不一致**：两轮 agent-browser 批处理显示点击成功但最终运行成功；这些轮次数据库无取消审计。另一次通过 Playwright 记录真实 IPC 的立即停止成功。当前仅证明自动化点击记录不足以证明取消请求送达，尚不能认定系统存在同样的取消缺陷。

## 已确认问题

### H-01：stdio 事件队列满时丢弃关键通知，连接仍表现正常

**级别：高（P1）；范围：stdio Harness 连接及其 SDK 消费者。** 尚无证据表明 Tauri 事件通道使用同一个队列。

#### 触发与复现

- 消费者暂时停止读取、输出管道阻塞或事件突发，令连接积压达到 256 条。
- `apps/cli/src/app_server.rs` 的 `EventOutbox::emit()` 在 `try_send()` 之前分配序号；队列满只向 stderr 输出错误，不关闭连接、不反馈生产者、不保留该通知等待重发。
- 默认忽略的审查探针 `audit_stalled_outbox_preserves_terminal_notification` 调用生产 `emit()`，将消费者固定在不读取状态，连续产生 256 条事件后发出终态，再恢复读取。
- 执行：`cargo test -p deepagent-cli audit_stalled_outbox_preserves_terminal_notification --offline -- --ignored --nocapture`。
- 实际输出：`dropped sequence 257`、`queued=256, terminal_received=false, next_sequence=258`；“必须收到终态”断言失败。该探针是确定性队列复现，并非声称此次真实 DeepSeek 桌面对话发生过管道阻塞。

#### 恢复链核对

- `event_ack()` 只保存进程内 `last_event_ack`，检查单调性；没有重发、缺口恢复或映射逻辑。
- Rust SDK `client.rs::spawn_reader()` 只把通知的 `params` 转为 `HarnessEvent`，丢掉 envelope 上的 `event_sequence`。订阅端不能据此直接检测服务器丢通知；SDK 自己的 broadcast 慢消费者可能收到 `Lagged`，那是另一层行为，不能混同。
- `thread/read` **可以**按 session/run 的持久化游标恢复事件及运行状态。因此不能称持久化历史已经丢失；但它不是 connection `event_sequence` 的重放接口，SDK 未自动触发或转换这条恢复链。

#### 成熟实现对照

仓库内 Codex 的 `借鉴/codex/codex-rs/app-server/src/transport.rs::send_message_to_connection()`：可断开的慢连接在队列满时调用 `disconnect_connection()`；不能断开的 writer 使用 `send(...).await` 施加背压。`transport_tests.rs::broadcast_does_not_block_on_slow_connection` 明确验证慢连接被断开、快连接继续收到通知。

`outgoing_message.rs::send_server_notification_to_connections()` 使用异步 `send()`。同文件也有允许丢失的 `try_send_server_notification()`，所以不能概括成“Codex 所有通知都绝不丢失”；可借鉴的是明确区分传递保证，并把慢连接故障显式化。

#### 后果和修复方向

- 终态丢失：只等通知的调用方可能一直认为运行中，即使数据库已终结。
- 审批通知丢失：客户端可能不知道需要批准，执行侧等待直到其他超时/取消机制介入。
- 增量丢失：客户端内容和真实结果不一致；没有自动恢复时必须主动读取历史。
- 修复应复用既有 `run_events`：定义可丢弃增量与必达控制事件；选用有界背压或显式断连/恢复提示；让 SDK 保留传输序号、区分传输和持久化游标，并测试恢复到终态。不能仅扩大通道容量。

### H-02：Harness ACK 与运行接纳分离，立即中断可能被虚假确认

**级别：高（P1）；范围：stdio `turn/start` / `turn/interrupt`；桌面 `start_chat_v2` 已采用更早的接纳，不能据此认定桌面同样失败。**

#### 触发与复现

- `ServerState::turn_start()` 只创建内存 `TurnState { active: true }`，返回 `started` 和待启动 `TurnLaunch`。
- `run_stdio()` 先写 start 响应，再 `spawn_turn()`；直到后台调用 AppCore 才进入运行接纳。因此客户端拿到 turn ID 时，运行取消注册可能尚不存在。
- `turn_interrupt()` 调用 `request_cancel()` 后只检查 `Result`，**忽略 `CancelRequest.accepted`**，无条件返回 `cancelling`。
- 探针 `audit_interrupt_after_start_ack_reaches_cancellation` 使用生产分发器，保留返回的 launch 以固定上述调度窗口，随后分发 interrupt 并核对生产取消注册和 RunStore。
- 执行：`cargo test -p deepagent-cli audit_interrupt_after_start_ack_reaches_cancellation --offline -- --ignored --nocapture`。
- 实际输出：`status="cancelling", active_registration=false, durable_runs=0`；取消已送达注册对象的断言失败。

这证明合法的 ACK 后/执行前交错中，控制请求可被确认但未送达取消对象。探针没有调用真实模型，也不证明每次真实快速点击都必现；窗口频率仍需端到端调度压力测试。现有 `turn_start_returns_turn_id_and_interrupt_routes_to_chat_cancellation` 只断言响应文字为 `cancelling`，未检验信号送达，因而无法覆盖该缺陷。

#### 成熟实现对照

Codex `借鉴/codex/codex-rs/app-server/src/request_processors/turn_processor.rs::turn_interrupt_inner()`：验证活动 turn，将请求登记为 pending interrupt，向核心提交 `Op::Interrupt`；普通 turn 的响应等待 `TurnAborted`，提交失败清理 pending 并返回错误。`turn_start_inner()` 将输入交给核心后才构造返回的 turn。

本系统不必照搬其 ACK 时机：桌面已经有 `ChatService::admit_run()`，在返回接纳响应前创建 durable run 和取消树。这条既有边界可供 Harness 复用，避免再建一套取消机制。

#### 后果和修复方向

- 用户或 SDK 相信停止已受理，但后台稍后仍可能发起模型请求、执行工具及产生费用/副作用。
- 只改响应文案或增加前端延时不能消除窗口。
- start 返回前完成统一接纳；执行端消费同一 admission；interrupt 必须依据 `accepted` 返回实际受理状态或保留可交付的取消意图，并覆盖立即取消、排队取消和完成竞争。
- 测试需检验“取消后没有模型/工具启动”与唯一终态，不能只验证 JSON 字段。

以上探针以默认 `#[ignore]` 保存，因为本轮仅审查，未修改生产行为；显式执行时预期为红色。修复对应业务问题时应改为默认运行的回归测试。

### H-03：Harness 的逐次沙箱 backend 选择未进入实际执行路由

**级别：高（P1）；范围：Harness `thread/start` / `turn/start` 的 `sandboxBackend` 覆盖。** 不等于所有桌面沙箱均无效，也不证明文件权限钩子失效。

#### 触发与复现

- `apps/cli/src/app_server.rs::turn_start()` 把请求或线程上的 `sandbox_backend` 放进 `HarnessRunOverrides`。
- `run_assembler.rs` 将 overrides 序列化给 `RunEnvironment::resolve()`；其 `RunConfigOverlay::apply_permission_profile()` 消费权限 profile 和 sandbox mode，却没有消费 backend 选择。
- CLI 的 `build_chat_service()` 在构造宿主时固定 `SandboxBackendCommandExecutor`。stdio 默认使用 `Direct`；`for_workspace()` 克隆已有 executor，没有依据逐次覆盖重新选择 backend。
- 审查探针在 `kernel_v2_e2e.rs::audit_sandbox_override_does_not_route_to_default_direct_backend` 中建立真实 ChatService 和 mock 模型工具循环，并注入记录型 Direct backend：**只记录调用并返回固定结果，不执行任何系统进程**。
- 设置 `WorkspaceWrite`、无交互审批策略以进入现有 executor 路由；逐次请求指定 `sandbox_backend="windows_sandbox"`。正确行为应选择指定 backend，或者在不可用时明确拒绝，不能继续交给 Direct。
- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e audit_sandbox_override --offline -- --ignored --nocapture`。
- 实际输出：`run_ok=true, direct_backend_calls=1`；“Direct 调用次数为零”断言失败。此为确定性 AppCore 路由复现，没有启动 Windows Sandbox，也没有进行真实主机逃逸操作。

#### 成熟实现对照

Codex 的 `app-server/src/request_processors/thread_processor.rs` 将请求 sandbox 转换进 `ConfigOverrides.sandbox_mode`，并从实际 config snapshot 提取 `sandbox_policy()` 返回；文件内 `collect_resume_override_mismatches()` 比较恢复请求与实际生效的沙箱配置。`core/src/exec.rs` 的执行路径继续携带 permission profile 和 sandbox 信息。可借鉴的是请求、有效策略和执行边界的一致性；不能把 Codex 的沙箱实现直接视作本系统的 Microsoft `.wsb` 后端。

#### 后果和修复方向

- 调用方明确要求隔离 backend，实际仍可能走宿主默认 Direct；属于安全选择与执行不一致。独立的审批、文件边界和命令规则仍然存在，不能据此推导它们全被绕过。
- 必须在现有 `SandboxBackend` 边界解析并验证逐次选择；不可用时显式失败，不能静默降级。
- 将生效 backend、工作目录、mode 和 network policy 形成逐次不可变执行上下文；保证工具注册和执行使用同一份结果。不要依赖修改全局共享 backend 来切换并发会话。
- 补充不同会话并行、继续会话、显式 backend 不可用及实际返回配置一致性测试。此探针同样默认忽略，待修复后转常规回归。

## 本次验证与交付边界

| 命令或实测 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo test -p deepagent-cli --offline` | 82 passed，2 个审查探针 ignored |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline` | 11 passed，1 个审查探针 ignored |
| `cargo test -p deepagent-cli audit_ --offline -- --ignored --nocapture` | 2 个探针明确失败，分别复现 H-01、H-02；不是常规测试通过 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_sandbox_override --offline -- --ignored --nocapture` | 探针明确失败，复现 H-03 |
| `git diff --check` | 通过 |

本轮新增内容限于报告和三个默认忽略的审查探针，未修改生产运行逻辑。没有重复运行全 workspace 测试、Clippy 或前端构建；本轮没有改变跨 crate 生产行为或前端代码。桌面使用当前源码真实编译产物测试，截图保存在忽略提交的 `target/live-desktop-test-20261021/`（`tool-result.png`、`restart-recall.png`）。

后续仍需覆盖插件 180 秒延迟故障注入、审计落盘失败、并发租约/取消竞争、完整压缩及重放、DeepSeek 流式异常与其他宿主入口。此次样本不能用来宣称全系统验收完成，或所有准备与取消问题均已修复。
