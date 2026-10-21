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
| stdio Harness / SDK 事件交付与恢复 | 队列丢终态、立即中断窗口、恢复丢覆盖配置均已确定性复现 | 已确认 H-01、H-02、H-04；其余路径未完成 |
| DeepSeek 请求、reasoning、工具协议、usage | 真实请求和 usage 返回；Chat 工具投影拆分与截断工具调用提交均已复现 | 已确认 H-05、H-18；全部错误和流式分支未完成 |
| 上下文、压缩、记忆 | 预取生命周期、压缩回放边界及无效压缩丢失已返回 usage 均已复现 | 已确认 H-10、H-11、H-14；笔记计数及其他压缩边界仍在核查 |
| 工具、审批、权限与沙箱 | 工作区拦截、backend override；迟到审批授权、跨运行审批 ID 冲突及子 Agent 工具限制未约束执行 | 已确认 H-03、H-15、H-16、H-19；其他边界未完成 |
| MCP、插件与 hooks 生命周期 | MCP 协议问题；AppCore 禁用插件后就绪快照与技能注入不同代已复现 | 已确认 H-06 至 H-09、H-12、H-13；hooks 及其他准备边界未完成 |
| 持久化、崩溃恢复、并发与输入租约 | 取消存储故障不挡信号；排队取消推进；真实桌面吞掉保存失败已复现 | 已确认 H-17；完整恢复及并发压力未完成 |
| CLI / SDK / 桌面 / 工作流与子 Agent 一致性 | 子 Agent 工具限制和 DAG 文件交付已复现缺陷 | 已确认 H-19 至 H-22；记忆现有回归通过，跨宿主和重启边界未完成 |

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
| MCP 初始化未完成时的普通对话 | 新增 `pending_host_mcp_initialization_does_not_block_conversations`：真实 Node MCP 子进程将 initialize 持有至 180 秒或显式释放；确认子进程已收到 initialize 后运行两轮真实 ChatService、mock 模型请求。两轮合计 384ms、initialize 仅 1 次、待就绪工具未发布且未进入任何模型请求；释放后工具正常发布。1 passed |
| 运行包准备等待真实磁盘锁时的普通对话 | `blocked_host_payload_preparation_does_not_block_conversations`：宿主 worker 确认进入生产磁盘准备路径并被真实 OS 文件锁阻塞；两轮 ChatService + mock 模型合计 349ms，运行包尚未解压、技能未发布；释放后成功解压，第三个新会话实际请求含就绪技能目录。1 passed |

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

### H-04：Harness 恢复与 steer 替换运行会清空显式模型和权限配置

**级别：高（P1）；范围：stdio `thread/resume` 后未重新指定覆盖值的运行。** 与 H-03 区分：本项影响权限 profile、模型和 provider 的生命周期，甚至同一宿主内重新附着也会触发。

#### 触发与复现

- 通过 `thread/start` 显式选择 `permissionProfile=read_only`、provider、model 和 sandbox backend；这些值仅存于进程内 `ThreadState`。
- `thread_resume()` 在确认会话存在后，无条件重新插入 `ThreadState`，将上述四项设为 `None`，包括线程已经加载在同一进程内的情况。
- `turn_start()` 只从本次参数或此 ThreadState 取值。恢复请求类型只有 `threadId`，也没有机会在恢复时重申或查看生效配置；启动响应只返回 `ready/cwd`。
- 探针 `audit_resume_preserves_explicit_thread_overrides` 使用生产请求分发器启动、恢复、再构造下一次 turn，检查交给 AppCore 的 overrides；不调用模型、不执行工具。
- 执行：`cargo test -p deepagent-cli audit_resume_preserves --offline -- --ignored --nocapture`。
- 实际输出：`before={"model":"deepseek-flash","permission_profile":"read_only","provider":"deepseek","sandbox_backend":"windows_sandbox"}, after={}`；配置保留断言失败。

#### 后续复现：不经 resume 的 steer 同样丢失配置

- `turn_steer()` 会取消旧运行、登记新 turn，并直接构造 `HarnessRunOverrides::default()`，既没有承接线程配置，也没有承接旧 turn 的有效覆盖值。
- `audit_steer_preserves_effective_turn_overrides` 通过生产 thread/start 与 turn/start 建立配置；显式调用既有 `admit_run()` 使取消注册和 durable run 确实存在，排除 H-02 的接纳窗口后再调用 steer。
- 执行：`cargo test -p deepagent-cli audit_steer_preserves --offline -- --ignored --nocapture`。实际 before 包含 provider/model/read_only/windows_sandbox 及 reasoning_effort=high，after 为 `{}`；保留断言失败。没有执行真实模型或工具。
- Codex `turn_processor.rs::turn_steer_inner()` 把新增输入交给现有线程的 `steer_turn()` 并验证 expected_turn_id；它不会通过创建默认配置的新运行实现此操作。本系统可以保留自己的“取消后替换 turn”语义，但必须让替换运行继承已解析的有效配置。
- 因此本项修复应覆盖 resume、steer、fork 等生命周期边界；只保存线程元数据仍不足以保留上一 turn 的逐次覆盖。

#### 成熟实现对照

Codex `app-server/src/request_processors/thread_processor.rs::collect_resume_override_mismatches()` 对已加载线程的恢复请求比较 model、provider、cwd、approval 与 sandbox 的请求值及实际 config snapshot；恢复响应包含实际配置。其 `ThreadResumeParams` 提供显式覆盖入口。本系统目前既无完整恢复配置、也无生效值回传。可借鉴的是恢复配置必须有可验证的语义，不要求无条件冻结所有历史设置。

#### 后果和修复方向

- 恢复后若宿主默认权限比 `read_only` 更宽，后续运行将回退到更宽配置；具体工具副作用还受宿主默认值与其他审批/钩子约束，此探针没有执行真实写操作。
- 模型或 provider 可变为宿主默认选择，影响能力、费用及数据发送目标；不能承诺同一 thread ID 仍代表相同执行条件。
- 在现有会话元数据中明确持久化可恢复的配置，或要求显式重新选择并返回有效配置；已加载线程不能在普通 resume 中悄悄丢弃配置。若希望始终采用最新全局设置，应在协议和响应中明确体现，并禁止静默放宽安全约束。
- 覆盖同进程重新附着、宿主重启、默认设置改变与更严格权限恢复；探针默认忽略，修复时转常规回归。

### H-05：Chat Completions 往返投影拆散同一次 assistant 工具响应

**级别：中（P2；启用严格 reasoning 校验的供应商可能升级为请求失败）；范围：`WireMode::ChatCompletions` 下经原生 ResponseItem 再发出的工具续轮。** 默认 Responses 路由的真实桌面测试不能证明本路径无误。

#### 触发与复现

- 模型同一条 assistant 响应同时返回 reasoning、说明文本和 tool call。这是正常工具协议形态。
- `ChatCompletionAccumulator::finish()` 将其转成 `Reasoning → FunctionCall → Message`；运行时再追加工具结果。
- 下次 `ResponseRequest::from_response_items()` 的配对修复把工具结果搬到 call 后，`client.rs::response_to_chat_completion()` 经兼容 `Message` 投影重新序列化，最终产生：

```text
assistant(reasoning + tool_calls, content="")
tool(result)
assistant(content="I will read the file.", 无 reasoning)
```

- 原本在执行前给出的说明变成执行后的独立 assistant 消息，且其 reasoning 不再跟随。工具 call_id 和 arguments 在本例仍完整，不能称它们也丢失。
- 探针 `client::tests::audit_chat_projection_preserves_tool_turn_narration_and_reasoning` 使用生产流式 accumulator、生产 request 构造器、生产 Chat 投影和 serializer；输入为合成但合法的单个工具响应，不访问供应商。
- 执行：`cargo test -p deepagent-models audit_chat_projection --offline -- --ignored --nocapture`。
- 实际 wire 显示上述四条消息（含最初 user）；assistant 数量为 2，期望保持同一响应的断言失败。

#### 官方规则与成熟实现对照

[DeepSeek Thinking Mode 官方文档](https://api-docs.deepseek.com/guides/thinking_mode/)（2026-10-21 核对）要求携带 tools 的后续请求完整回传历史 reasoning，包括没有调用工具的 assistant 回答；示例将同一 response 的 content、reasoning 和 tool_calls 保留在同一个 assistant message 中。这里已证明 wire 关联被改变，**尚未用此特定请求实测 HTTP 400，不能把潜在拒绝当作已发生的网络结果**。

DeepSeek Harness `packages/llm/llm-deepseek/src/serialize.ts::assistant()` 将同一 durable assistant message 中的 text、reasoning、tool-call 块统一映射为内容；`replay.ts` 以块索引校验模型和原生 metadata 的对应关系。它当前走 Messages 协议；借鉴点是保持 assistant turn 的边界与块归属，不是复制字段名。

#### 后果和修复方向

- 续轮上下文含额外、时间顺序错误的 assistant 发言，破坏模型对工具执行前后语义的理解；严格 reasoning 规则下存在请求被拒绝风险。
- native Surface 与 Chat adapter 之间不能把有损 UI 投影当成协议回放。应由 provider seam 保留或恢复同一响应的块归属，在一次 serializer 内生成正确的 assistant 原子消息。
- 补充含文本的单/多工具响应、reasoning 多块、重启回放、压缩边界和取消中断测试，并核对真实 wire；不能通过填造 reasoning 来满足校验。

### H-06：MCP 断线恢复自动重发结果未知的工具调用，可能重复产生副作用

**级别：高（P1）；范围：使用 `ReconnectingTransport` 的 MCP 调用。** 不是一般的“连接重建有问题”，而是连接恢复与业务操作重试没有分开。

#### 触发与复现

- `reconnect.rs::ReconnectingTransport::send()` 遇到字符串匹配的 `broken pipe`、`connection lost` 等错误后，重新连接并对新 transport 调用 `send(request)`；既不区分 `tools/list` 与 `tools/call`，也不知道旧调用是否已在服务端执行。
- `mcp_service.rs::connect_one_resilient()` 将生产连接包装为此 transport；`McpClient::call_tool()` 没有额外幂等键或结果去重。JSON-RPC request ID 是响应关联标识，不能保证重启后的外部服务不重复执行。
- 探针 `reconnect::tests::audit_disconnect_after_tool_effect_does_not_replay_mutation` 通过真实 McpClient 调用合成 `append_record` 工具。第一个 mock transport 先增加副作用计数，再返回 `broken pipe` 模拟响应丢失；重连后的 transport 增加同一计数并成功返回。没有执行真实外部写操作。
- 执行：`cargo test -p deepagent-mcp audit_disconnect_after_tool_effect --offline -- --ignored --nocapture`。
- 实际输出：`result_ok=true, applied_effects=2`；期望只执行一次的断言失败。客户端看到成功，但逻辑操作已经执行两次。现有重连测试只模拟执行前断线，未覆盖执行成功但回包丢失。

#### 成熟实现对照

Grok 的 `xai-grok-shell/src/session/mcp_restart.rs::auto_restart_stdio()` 使用 1/4/16 秒有界退避重建并发布连接；该重启任务不持有原工具请求，也不负责重发它。本系统文件注释虽称对齐此机制，却增加了语义不同的业务请求重试。

Codex `rmcp-client/src/rmcp_client.rs::run_service_operation_with_transient_retries()` 明确只对 `tools/list` 的可重试传输错误做通用重试，`is_retryable_tools_list_error()` 对其他方法返回 false。它也支持显式 HTTP session-expired 404 的恢复，不能概括成“Codex 从不重试任何工具”；关键差别是没有把任意断线都解释成可以重发副作用。`tool_input.rs::call_tool()` 对已提交 proof 的续轮失败还明确禁止重启原操作。

#### 后果和修复方向

- 非幂等工具可能重复新增记录、发送消息或创建外部任务；这些是影响示例，不表示此次测试操作了这些真实服务。重试全部耗尽时，还可能出现客户端报错、服务端已执行多次的状态。
- 将连接恢复与操作重试分离：为后续调用恢复连接；已发出且结果未知的调用返回结构化 unknown-outcome，让运行知道需要核实结果。
- 只有明确未发送、操作被证明幂等或服务端支持持久化幂等键时才允许自动重发。不能仅依据错误字符串或只读名称推断安全。
- 补充执行前断线、执行后回包丢失、并发重连、取消与多次断线测试，并验证调用次数和最终外部状态。此审查探针默认忽略，生产逻辑尚未修复。

### H-07：MCP 工具发现忽略分页，只发布首页工具且不报不完整

**级别：中（P2）；范围：返回 `nextCursor` 的 MCP 服务。** 不分页的服务不受此项影响。

#### 触发与复现

- `McpClient::list_tools()` 只发一次不带 cursor 的 `tools/list`。`protocol.rs::ToolsListResult` 只有 tools 字段，反序列化直接丢弃 `nextCursor`。
- `McpRegistry::register_with_instructions()` 及 AppCore 工具发现继续使用该结果，成功注册并发布首页集合，没有第二条生产路径补拉后续页。
- 探针 `client::tests::audit_tool_discovery_reads_all_pages` 使用两页合规 mock server：第一页工具 first，并返回不透明 cursor；携带同一 cursor 才返回 second。通过真实 McpClient 发现工具。
- 执行：`cargo test -p deepagent-mcp audit_tool_discovery_reads_all_pages --offline -- --ignored --nocapture`。
- 实际输出：`names=["first"], requests=[None]`；期望 `["first","second"]` 的断言失败。不是目录展示截断，而是后续工具根本没有被发现和注册。

#### 官方规则与成熟实现对照

[MCP 2024-11-05 分页规范](https://modelcontextprotocol.io/specification/2024-11-05/server/utilities/pagination)明确将 tools/list 列为支持分页的操作，以 nextCursor 继续、缺省表示结束；cursor 必须作为不透明值传回。

Codex `codex-mcp/src/rmcp_client.rs::list_tools_for_client_uncached()` 用 `pagination.rs::collect_paginated_with_limit()` 汇总全部页，同时限制页数、条目数、cursor 大小和整体耗时，并拒绝重复 cursor。可以借鉴其“完整发现且有界”的边界，不能仅增加一次无界循环。

#### 后果和修复方向

- 模型会误认为插件没有后续页的能力，任务无法完成或选择替代工具；设置界面还可能显示连接正常，增加定位难度。
- 在现有 McpClient 统一实现有界分页，供 registry、状态查询和就绪快照共用；分页失败不能把半个工具集当作完整成功发布。
- 补充无分页、多页、空页但有 cursor、重复 cursor、超限和取消测试。此探针默认忽略，生产发现逻辑尚未修复。

### H-08：MCP 初始化缺少 initialized 通知，严格服务端无法进入就绪状态

**级别：中（P2；对要求完整握手的服务为连接阻断）；范围：当前 McpClient、stdio 及使用同一握手的重连路径。** 宽松服务端可能仍工作，不能概括成所有 MCP 都无法连接。

#### 触发与复现

- `McpClient::initialize()` 仅发送 initialize 请求并直接返回其 result；没有发送 notifications/initialized。
- `McpTransport` 只提供带 u64 ID 的 request/response `send()` 与 close，没有无 ID 通知接口；stdio 和 HTTP 实现都没有在初始化响应后补发通知。AppCore 的初连及 ConfigReconnectFactory 也未补足此步。
- 真实子进程探针 `tests/protocol_audit.rs::audit_strict_stdio_server_completes_initialization` 启动本机 Node JSON-RPC fixture。它接受本系统声明的协议版本，initialize 成功，但在 initialized 通知前拒绝 tools/list。探针使用生产 StdioTransport 与 McpClient，并在断言前关闭子进程。
- 执行：`cargo test -p deepagent-mcp --test protocol_audit audit_strict_stdio --offline -- --ignored --nocapture`。
- 实际服务端记录：`received=initialize,tools/list`，返回 `not initialized`；发现成功断言失败。该失败来自有意严格的本机 fixture，未声称所有用户插件已返回此错误。

#### 官方规则与成熟实现对照

[MCP 2024-11-05 生命周期规范](https://modelcontextprotocol.io/specification/2024-11-05/basic/lifecycle)要求 initialize 成功后客户端发送 initialized 通知，表示可以开始正常操作。本系统声明的正是这一协议版本，因此不是新版本功能缺失。

Codex `core/tests/suite/codex_apps_protocol.rs` 将 legacy 握手次序断言为 `initialize → notifications/initialized → tools/list`；`rmcp-client` 通过服务生命周期驱动握手，不用裸 request 代替完整初始化。其现代协议有独立协商分支，不能据此跳过本系统所选 legacy 协议的必需步骤。

#### 后果和修复方向

- 严格服务端无法发布工具，插件可能被标记连接/发现失败；仅重试会重复不完整握手，增加等待而不能恢复。
- 在现有 transport/client 边界补齐 JSON-RPC notification 和初始化状态机，只有协议协商及通知完成后才能发布 Ready。初连、显式重试和断线恢复共用这一实现。
- 补充 wire 次序、通知无 ID、不等待通知响应、初始化失败不发通知、协商版本不支持及重连握手测试。此探针默认忽略，生产握手尚未修改。

### H-09：stdio MCP 并发回包按读取者筛选，其他请求的响应会被永久丢弃

**级别：高（P1）；范围：同一个 StdioTransport 上存在两个未完成请求且服务端乱序响应。** 单请求顺序测试无法发现此问题。

#### 触发与复现

- `StdioTransport::send()` 分别锁 stdin 和 stdout：请求 A 写入后持有 stdout 等待；请求 B 仍能写入，但要等待 stdout。A 的读取循环遇到 ID 不等于 A 的响应就直接跳过，没有按 ID 缓存或分派。
- 服务端先回复 B、再回复 A 时，A 丢弃 B 的有效响应后完成；B 获得 reader 后再也读不到自己的响应。无外层超时则持续等待；有超时则把已成功的调用报为超时。
- 现有 registry/liveness 共享 Arc<McpClient>，后台 `client.ping()` 能与工具请求重叠；共享服务的多会话调用也需要此 transport 正确分派。此缺陷不以“模型一定并行调用 MCP 工具”为前提。
- 真实 Node 子进程探针 `stdio::tests::audit_out_of_order_concurrent_replies_reach_both_callers` 使用生产 StdioTransport。先观察请求 A 已持有 stdout，再发送 B；fixture 收齐两请求后逆序写回两个结果。用锁状态固定交错，不以任意 sleep 猜时序。
- 执行：`cargo test -p deepagent-mcp audit_out_of_order --offline -- --ignored --nocapture`。
- 实际输出：A 的 id=10 响应正确，B 为 `Err(Elapsed(()))`，尽管服务端已写出 id=11。探针在断言前关闭子进程；没有访问真实插件或执行副作用。

#### 成熟实现对照

Codex `rmcp-client/src/rmcp_client.rs::call_tool()` 从 RunningService peer 提交请求，得到关联的 request handle 后单独 await_response；连接服务与每个调用的结果等待分离。Codex 的另一条 JSON-RPC 连接 `app-server-client/src/remote.rs` 明确用 pending_requests 将 response.id 分发给对应的 oneshot sender。后者不是 MCP transport，借鉴的是相同 JSON-RPC 关联机制，不能混称为 MCP 的实现文件。

#### 后果和修复方向

- 正常成功的工具看起来挂起/超时；活性探针也可能误判健康连接，进而触发不必要重连。与 H-06 叠加时存在未知结果被再次执行的风险；本探针只验证丢响应，没有实测这条组合故障。
- 在现有 transport 内收敛成单 reader 驱动，按 ID 投递各 pending request；通知走独立处理，取消移除对应 waiter，断线使全部未完成请求得到明确错误。所有连接用户共用该分派器。
- 只锁住整个 send 可以避免此示例丢包，但会让慢工具阻塞活性检查和其他调用，不能当作完整并发设计；不要简单延长 timeout。
- 覆盖乱序、交错通知、超时后迟到响应、单请求取消、断线与并发 close。此探针默认忽略，生产 transport 尚未修改。

### H-10：运行结束后预取压缩任务仍继续执行，结果及 usage 无处归集

**级别：中（P2）；范围：进入 prefire 提前压缩区间、主运行先结束而 pass-1 尚未返回的场景。** 短上下文未触发 prefire 时不受此项影响；不同于有明确独立生命周期的后台 sub-agent。

#### 触发与复现

- `ModelAgent::maybe_schedule_prefire()` 直接 tokio::spawn，保存在普通 JoinHandle 中；任务不接收运行取消树，也没有 Drop/终态收尾取消或转交宿主的路径。
- AppCore `run_assembler.rs` 为生产 agent 配置 prefire_start，并在运行收尾读取 `agent.side_usage()`。该方法只包含已归集的 side_usage 和 prefire_cache，不包含尚未完成的 handle。
- 主运行完成或取消后丢弃 agent，只丢弃 JoinHandle，并不会取消已 spawn 的任务。它后来产生的 PrefireNote 只作为孤立 task 的返回值，不会进入后续运行、持久化缓存或已结束的计费记录。
- 探针 `model_agent::tests::audit_completed_agent_does_not_leave_prefire_running` 使用生产 ModelAgent 调度、mock 主模型和可控的 compactor：将提前阈值设为 1、硬阈值设为 1,000,000，确保进入 lead band；主模型正常结束，确认 pass-1 已开始，再读取 side_usage 并释放 agent，最后允许 pass-1 完成。没有请求真实供应商。
- 执行：`cargo test -p deepagent-runtime audit_completed_agent_does_not_leave_prefire_running --offline -- --ignored --nocapture`。
- 实际输出：`main_done=true, continued_after_owner_end=true, side_usage_at_end=None`；要求释放 agent 后不继续执行推测请求的断言失败。该探针直接复现正常结束，不是一次真实桌面点击取消；取消路径同样未向此 task 传递信号的判断来自生产调用链。

#### 成熟实现对照

Grok `session/compaction_config.rs::CompactCancelGate` 让 compact/prefire 共享取消 token；`compaction.rs` 将 token 传给 `generate_session_compact()`，`acp_session_impl/cancel.rs` 在停止时调用 `compaction.cancel.request_cancel()`。模型切换/记忆控制失效时，`model_switch.rs::abort_and_clear_prefire()` 还会取出 handle、abort 并 await，再清理缓存。

Grok 可以让部分预取跨 turn 保留，因为它的 handle/cache 属于 SessionActor；本系统却放在逐运行的 ModelAgent 中。不能只复制 spawn 和两段压缩策略，遗漏任务所属生命周期及费用归集。

#### 后果和修复方向

- 主界面已经完成或停止后，推测摘要仍可能占用网络、模型配额和费用；本地账单无法归集晚到 usage。取消本地请求也不能保证供应商已产生的费用为零，修复不能作此承诺。
- 运行反复进入提前区间后立即结束，会重复启动无消费者的摘要；本次未做真实供应商批量压力或费用测量。
- 在既有取消树中给运行所属的摘要/预取派生子信号，终态收尾有界取消并回收；若要跨 turn 缓存，应由现有会话服务明确接管其身份、有效性和计费，不能丢弃 handle 后默许继续。
- 已获得的 usage 必须在有主人的收尾路径归集；覆盖正常完成、用户取消、模型切换、摘要已完成但未收取、任务失败及会话重启。此缺陷探针默认忽略，生产行为尚未修改。

### H-11：以消息内容反查压缩边界，重复回答会让恢复重新带回已摘要的历史

**级别：中（P2）；范围：原生压缩保留尾部的首条可定位 item 是重复 assistant 文本等非唯一内容。** 用户消息已有标签，不能概括为所有重复用户输入都会触发；本项与 H-05 的 Chat 投影拆分无关。

#### 触发与复现

- `context_runtime.rs::record_native_boundary()` 用 tail 中第一条可解析 item 查找事件序号；`input_runtime.rs::sequence_of_response_item()` 对完整事件日志从前向后执行内容相等匹配，没有保留该 live item 的事件身份。
- 相同的 assistant `Done` 可以出现在早期与近期，查找会选中早期事件。`ContextCompacted.first_kept_sequence` 因而指向已经被摘要的前缀；恢复按该序号重放，把前缀与新摘要同时放回 Surface。
- 新增探针 `context_runtime::tests::audit_native_summary_replay_preserves_repeated_assistant_boundary`：使用真实内存数据库、Session 原生事件、生产 keep-window 选择器、HookedReactiveContextCompactor 和恢复函数，只有摘要模型使用 mock。24 轮旧对话及近期尾部中包含两条相同的 assistant `Done`；通过断言确认生产选择器确实从近期的 `Done` 开始保留 5 条，而非人为指定一个无法出现的切点。
- 执行：`cargo test -p deepagent-app-core audit_native_summary_replay_preserves_repeated_assistant_boundary --offline -- --ignored --nocapture`。
- 实际输出：`expected=50, saved=Some(2), original_items=54, live_items=6, replay_items=54`；live 与 replay 相等的断言失败。压缩后应只剩摘要 + 5 条尾部，恢复却多出 48 条已被摘要的旧内容。日志原始事件未删除，问题是模型视图的恢复边界错误。
- 这是生产压缩及数据库回放的确定性复现；尚未做真实供应商大窗口压缩后重启桌面的端到端复测。既有 `native_summary_keeps_exact_tool_metadata_across_replay` 使用可区分的用户尾部，无法覆盖这个非唯一边界。

#### 成熟实现对照

Claude Code `services/compact/sessionMemoryCompact.ts` 通过 `lastSummarizedMessageId` 与 `msg.uuid` 找到已总结位置；已知 UUID 不存在则返回 null，转入其他压缩路径，不拿相同文本猜边界。随后统一选择尾部并保护工具配对。

Codex `core/src/session/mod.rs::replace_compacted_history()` 先为缺失身份的 item 分配 ID，再从同一份 items 建立 live history 与持久化 `CompactedItem.replacement_history`；`core/src/compact.rs::CompactedHistoryMetadata` 的注释明确要求二者一致。它持久化的是实际替换结果，不依赖重启时凭文本猜原事件。

#### 后果和修复方向

- 跨运行或重启后，模型看到“摘要 + 被摘要的原文 + 近期尾部”，token 和费用增加，可能很快再次进入压缩或 overflow；摘要已更新的状态还可能与恢复的旧叙述冲突。这些是错误 Surface 的推论，本次没有测实际收费或真实模型误判。
- 原生 Surface 应携带既有事件 sequence/稳定 item 身份或旁路 provenance，压缩尾部必须定位到确切事件；不能简单把 first match 改成 last match，因为尾部内部也可能重复。
- 沿用现有 `ContextCompacted` / `run_events` 持久化边界，必要时兼容扩展实际 replacement/provenance，不能新建第二套上下文存储。来源不可验证时明确失败或采用可验证的保守边界，不写一个猜测的成功切点。
- 补充相同 assistant 文本、相同 reasoning、连续多次压缩、snip/patch 后压缩及重启后的 live/replay/wire 等价测试。此探针默认忽略，生产边界尚未修复。

### H-12：Streamable HTTP 未保存初始化分配的 session header，有状态远程服务后续请求失败

**级别：中（P2）；范围：启用 http feature、使用初始化分配 Mcp-Session-Id 的 Streamable HTTP 服务。** 无状态 HTTP 服务及 stdio 不受这项 header 缺失影响。

#### 触发与复现

- `connect.rs::connect_network()` 的生产 HTTP/SSE 路由进入 `http.rs::HttpTransport`；该结构只保存静态 URL、配置 headers 和 reqwest Client。send 读取响应状态及 Content-Type 后消费正文，丢弃初始化返回的会话标识，下一请求只能发送静态配置头。
- 新增 `tests/protocol_audit.rs::audit_http_initialization_retains_session_header`：在 127.0.0.1 的随机端口启动真实 TCP/HTTP fixture，第一次 initialize 返回 JSON 及 `Mcp-Session-Id: audit-session`，第二次 tools/list 只有带回该头才成功，否则返回规范建议的 400。请求均经过真实 reqwest / HttpTransport，不是 mock send。
- 执行：`cargo test -p deepagent-mcp --features http --test protocol_audit audit_http_initialization_retains_session_header --offline -- --ignored --nocapture`。
- 实际输出：`received=[("initialize", false), ("tools/list", false)]`，`discovery=Err(...400 Bad Request: missing Mcp-Session-Id)`；后续请求携带初始化 session 的断言失败。
- 最终探针直接驱动生产 HttpTransport，initialize 的请求及响应版本均为 2025-03-26，排除 McpClient 仍固定声明 2024-11-05 的版本协商混杂因素。fixture 仅检查 session header，不检查 initialized 通知，以隔离 H-08；不是一次完整 MCP 协议符合性或外部服务验收。

#### 官方规则与成熟实现对照

[MCP 2025-03-26 Streamable HTTP 规范的 Session Management](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports#session-management)允许服务器在初始化时分配 Mcp-Session-Id，随后客户端必须带回；缺失时服务可返回 400，过期会话返回 404 并要求重新初始化。此处依据 Streamable HTTP 规则，不能把这些字段说成 2024-11-05 legacy HTTP+SSE 的原有要求。

Codex `rmcp-client/src/http_client_adapter.rs` 从 HTTP response headers 读取 HEADER_SESSION_ID，并将 session_id 与 Json/Sse 响应交给 rmcp StreamableHttpClientTransport；后续 post_message 接收该 session_id，并通过同一适配器插入 mcp-session-id 请求头。遇到过期会话另有恢复边界，不是要求用户手填一次性标识。

#### 后果和修复方向

- 初始化 HTTP 本身成功，但后续工具发现或调用被拒，远程工具无法进入就绪快照；不能把错误仅归因于 API Key 或服务宕机。本次只测试本机 fixture，没有使用用户凭据或外部 MCP。
- 在现有 transport/client 边界拥有动态会话状态，安全读取并带回标识；配置凭据与 session header 分开，错误及日志不得泄漏真实会话标识。
- 同时明确支持的协议版本、协商/拒绝规则及 legacy SSE 的不同连接流程；当前固定的 2024-11-05 版本不应让代码注释中的 Streamable HTTP 支持声明替代真实兼容测试。
- 补初始化、普通调用、ping、并发请求、过期 404、重建及关闭测试；恢复会话不能绕过 H-06 的副作用未知结果限制。此探针仅在 http feature 下编译，默认忽略，生产 transport 尚未修复。

### H-13：AppCore 插件就绪快照与共享技能注册表不同步，新请求可继续声明已禁用技能

**级别：中（P2）；直接复现范围：使用公共 PluginService + ChatService 生命周期的宿主，在禁用后立即创建新会话。** 桌面 `set_plugin_enabled` command 额外同步 skills，该命令返回后的路径不能据此判定同样失败。

#### 触发与复现

- `PluginService::set_enabled()` 保存禁用设置并执行 `PluginPreparation::invalidate()`，立即清空 ready snapshot；但 SkillsService 是另外一份共享注册表，不随此次失效立即更新。
- `ChatService::start_plugin_preparation()` 仅在后台 changed 回调时用 ready skill_roots 更新 SkillsService。`build_skill_catalog_blocks()` 与 `tool_runtime::register_skill_tool()` 直接从后者读目录/复制 registry，没有核对该请求捕获的插件 ready generation。
- 探针 `chat_service::tests::audit_disabled_plugin_skills_do_not_enter_new_model_request`：创建真实临时插件和 SKILL.md，启动生产宿主准备，等待首次就绪及技能发布；调用公共 set_enabled(false)，确认 ready skill_roots 已为空，再通过真实 ChatService/Kernel 发起全新会话，截获模型的 TransportRequest。模型只有离线固定回复，未使用外部 API。
- 执行：`cargo test -p deepagent-app-core --lib audit_disabled_plugin_skills_do_not_enter_new_model_request --offline -- --ignored --nocapture`。
- 实际输出：`enabled=false, ready_skill_roots=0, new_request_has_disabled_skill=true`；新请求不应包含禁用技能的断言失败。该会话没有历史，不能用“保留过去的技能调用记录”解释本次新注入。
- 后续收紧探针：插件 fixture 放在独立聊天工作目录之外，仅匹配完整目录条目 `- audit-plugin-skill: Audit plugin readiness`，再次运行仍得到相同失败。排除动态环境中的普通文件路径命中技能名造成误报；不是仅据 body 中出现一个文件夹名称定性。
- 桌面 `lib.rs::set_plugin_enabled()` 在返回前调用 `sync_plugin_runtime_after_change()` / `sync_plugin_skill_roots()`，是一个宿主特有的补同步路径。本探针明确没有调用这层 Tauri handler；文件 watcher 失效也可能经过共享注册表更新窗口，但尚未独立做 watcher → 桌面 wire 复现，不能扩大结论。

#### 成熟实现对照

Claude Code `services/plugins/pluginOperations.ts::setPluginEnabledOp()` 在写 enabledPlugins 后统一调用 `utils/plugins/cacheUtils.ts::clearAllCaches()`，后者清插件、命令、Agent 及 skill prompt cache，并 resetSentSkillNames。它把失效组织在插件操作边界，避免要求每个 UI/CLI 调用方自行记住一套附加同步。

该参考实现对 hook pruning 仍有异步生命周期，源码注释明确暂时保留旧 hooks 直到 prune 完成；这里不声称其所有能力在所有时刻都是原子替换。采用的边界是统一失效入口和后续请求重新读取有效能力。

#### 后果和修复方向

- 新请求向模型声明已经不就绪/禁用的技能，造成模型选错能力；register_skill_tool 也可能复制旧 registry。探针直接证明新目录注入，没有调用禁用技能或验证任何权限绕过。
- 禁用与能力失效属于 AppCore 插件生命周期，不能只在 Tauri command 补同步。使该请求的 skills catalog、SkillTool registry、MCP/hook/Agent 投影来自同一份可验证就绪代次，或在统一边界进行同步失效/快照读取。
- 不为解决竞态重新把插件校验和连接放回会话路径；未就绪能力应缺省不可见。补 AppCore/CLI/Desktop、watcher、禁用与新请求交错、多插件及准备失败用例。此探针默认忽略，生产逻辑未修改。

### H-14：无效压缩保留原上下文时，已完成摘要请求的 usage 一同被丢弃

**级别：中（P2）；范围：同步原生压缩已取得 provider usage，但替换结果不比原文更小。** 与 H-10 的后台任务脱离生命周期不同，本项摘要已经完成并返回；prefire note 未采纳时另有记账分支，不能概括为所有失败压缩都漏记。

#### 触发与复现

- `HookedReactiveContextCompactor::compact_response_items()` 取得 `(summary_block, summary_usage)` 后比较 tokens；若 `tokens_after >= tokens_before`，直接返回 `Ok(None)`，usage 没有返回通道。
- `ModelAgent::maybe_proactive_compact()` 仅在 `Some(ReactiveCompaction)` 的 adoption 中归集同步摘要 usage；`None` 只增加失败计数。生产 `run_assembler.rs` 收尾读取 `agent.cumulative_usage()` / `agent.side_usage()`，后者无法看到已经丢掉的摘要用量。
- 探针 `context_runtime::tests::audit_ineffective_native_compaction_still_accounts_summary_usage` 使用真实 ModelAgent、生产 HookedReactiveContextCompactor、keep-window 选择器和内存 Session 数据库；主模型和摘要模型分别使用可观察离线 transport。历史为一个很短的前缀加足够大的近期尾部，断言生产选择器确实只压缩短前缀；摘要是正常短 JSON，没有超过 1024 输出上限的大型假回复。
- 执行：`cargo test -p deepagent-app-core --lib audit_ineffective_native_compaction_still_accounts_summary_usage --offline -- --ignored --nocapture`。
- 实际输出：`model_calls=2, summary_usage=120+18, main_billed=120, side_usage=None, adopted=false`。摘要及主模型均成功返回 120 输入 / 18 输出，下一主请求仍包含原文，且未写入 ContextCompacted；要求 side_usage 收到摘要用量的断言失败。
- 这直接证明 Agent 对运行收尾提供的用量缺失；没有请求真实供应商，也没有把测试的固定 token 数当作实际收费金额或验证桌面账单截图。

#### 成熟实现对照

Codex `core/src/compact.rs` 在压缩流收到 `ResponseEvent::Completed` 时，先调用 `record_observed_response_completed()` 及 `update_token_usage_info()`，然后才返回 CompactionResponse 给后续替换路径。已观察到的用量归集与是否最终采用 replacement 分离。

本系统底层 `ModelCompactor::summarize_accounted()` 也已经在 JSON 解析或 heuristic fallback 之前归集 usage；缺口在上层把“没有可采用的替换结果”和“没有发生模型消耗”都表达成了 None。

#### 后果和修复方向

- 运行及会话的模型用量少计已经完成的摘要请求；反复无效压缩会继续消耗而本地统计不完整。外部供应商仍可能正常收费，不能把保留原文解释为请求免费。
- 在既有 compactor outcome / 运行计费边界里分别携带“已观察用量”和“可选 replacement”，或通过同一运行所属的用量收集器归集；不能只在 adoption 成功时记账，也不能新增第二套账本。
- 保留摘要必须更小的保护，不通过强行采用更大的摘要修复记账；同一请求用量只能归集一次。后续覆盖无效摘要、post-hook 拒绝/失败、持久化失败、fallback、prefire stale、用户取消和重试。
- 此缺陷探针默认忽略，生产行为未修改；hook/持久化失败的漏记属于相邻待验证分支，本次没有据此独立定性。

### H-15：审批等待被取消后未清理，运行终态后仍接受迟到回答并授予跨调用权限

**级别：高（P1）；范围：工具已进入人工审批，用户取消运行，然后对旧审批提交 tool_session 等扩大作用域回答。** 本次验证取消本身有效、命令没有执行；问题是已结束运行的审批生命周期及迟到授权。

#### 触发与复现

- `ToolPipeline::request_approval()` 用 select 等待审批或运行取消。取消分支把 action 记为 Cancelled，但丢弃 ChannelApprovalGate future 时，没有删除 PendingApprovals 中保留的 sender。
- `RunStore::finish_with_event()` 写 cancelled 终态，没有同时使该 run 的待处理审批失效。NewRunApproval 的 expires_at 为 None，记录仍是 Pending。
- `RunCoordinator::resolve_approval_scoped()` 只验证审批记录及 offered scopes，没有拒绝所属 run 已终结的回答。数据库写入成功后，`PendingApprovals::resolve_scoped()` 即使 send 失败（receiver 已随取消丢弃），仍记录 tool_session grant；协调器因存在 durable record 返回 Ok(true)。
- 探针 `tests/kernel_v2_e2e.rs::audit_cancelled_run_rejects_late_approval_and_releases_waiter` 使用真实 ChatService 接纳、Kernel、审批 gate、运行取消树和内存 SQLite；离线模型发出 bash 调用，命令 executor 为记录型拒绝 fixture，绝不创建系统进程。等待真实审批通知后取消，确认唯一 cancelled 终态及 action=Cancelled，再通过公共 resolve_approval_scoped 提交旧回答。
- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e audit_cancelled_run_rejects_late_approval_and_releases_waiter --offline -- --ignored --nocapture`。
- 实际输出：`terminal=cancelled, action=cancelled, command_calls=0, pending_after_terminal=1, durable_state=Pending, late=Ok(true), granted_after_terminal=true, last_event=approval.responded`。要求释放审批并拒绝迟到权限的断言失败；还在 run_terminal 之后追加了旧审批的 answered 事件。
- 这是 AppCore 真实审批/取消/持久化链复现，没有点击真实桌面旧弹窗，也没有执行后续真实命令。不能据此声称已经停止的命令被重新启动。

#### 成熟实现对照

Codex `core/src/state/turn.rs` 把 pending_approvals 放在 TurnState；`clear_pending_waiters()` 清审批、用户输入等 waiter。`core/src/tasks/mod.rs` 取消任务后通过 `session/input_queue.rs::clear_pending()` 清理该 turn 的等待对象；注释强调先让任务观察取消，再清 waiter，避免把中断误投影成模型可见的用户拒绝。

可以借鉴运行/turn 所属的审批生命周期及有序清理，不能通过全局 clear 影响其他并发会话。持久化审批过期还需本系统沿用现有 run_approvals/run_events 完成。

#### 后果和修复方向

- 停止后旧审批仍占用共享注册表和数据库 Pending 状态；迟到回答被报告受理，能留下跨调用授权。ChannelApprovalGate 后续对相同 tool/risk 会读取该 grant，可能减少下一调用的人工提示；本次没有独立执行下一调用验证 UI，也不等于其他文件/沙箱权限被绕过。
- 在现有终态收尾中按 run 失效/清理待处理审批，与接收迟到回答共享一个可验证状态边界；answer 处理应核对 run/action 仍允许等待，不能仅检查 approval_id 存在。
- waiter 需要携带 run/turn 身份并具备取消/drop 清理，持久化投影同步过期；范围授权只能由仍有效的审批产生。不要用全局清空、静默吞 send 失败或延迟按钮遮盖问题。
- 覆盖审批期间停止、回答/取消竞态、重复回答、工具作用域与命令前缀持久化作用域、两会话隔离、进程重启及审计写失败。本项只验证 tool_session；command_prefix 采用同一路径但尚未独立复现，不把它当作已经执行的测试。
- 此缺陷探针默认忽略，生产审批逻辑未修改。

### H-16：审批直接以 provider call_id 为全库身份，不同运行之间产生幂等冲突

**级别：中（P2）；触发条件：两个独立运行收到相同的 provider tool-call id，且都需要审批。** 不声称真实供应商在本次桌面样本中返回了重复 ID；这是宿主在缺少全库唯一保证时的身份边界缺陷，与 H-15 的终态清理不同。

#### 触发与复现

- ModelAgent 将 provider 的调用标识原样放入 ToolInvocation.id；`ToolActionPersistence::register_invocations()` 正确使用 `(run_id, call_id)` 检查和持久化 action，仅缺失 id 时生成新值。
- 同一结构的 `request_approval()` 却直接令 `approval_id=call_id`；`run_approvals` 以 approval_id 唯一。第二个 run 的 action 能正常登记，审批则撞到第一条记录；RunControlStore 检查 run 不同后返回 idempotency conflict，不会建立第二条待审批记录。
- 探针 `tool_pipeline::tests::audit_distinct_runs_can_approve_the_same_provider_call_id_independently` 使用真实内存数据库和生产 ToolActionPersistence：第一项审批拒绝后，另一独立 session/run 使用相同 provider id 请求自己的审批。两个会话没有共享模型历史，排除同一请求内重复 tool id 的混杂因素；工具仅为计数 fixture，测试没有 invoke 或模型网络调用。
- 执行：`cargo test -p deepagent-runtime audit_distinct_runs_can_approve_the_same_provider_call_id_independently --offline -- --ignored --nocapture`。
- 实际输出：`first_state=Denied, second_action=Received, second_approval=Err(Invalid("idempotency conflict for approval provider-call-reused")), first_approvals=1, second_approvals=0`；第二运行应有独立审批的断言失败。
- 存储冲突检查在这里正确地阻止了覆盖/复用旧授权；本探针证明合法新 action 无法进入审批，不证明旧会话的一次批准已错误放行另一个会话。

#### 成熟实现对照与修复方向

- Codex `core/src/session/mod.rs::request_command_approval()` 将 effective_approval_id 的 waiter 放进当前 active turn 的 TurnState，并说明匹配必须投递到正确的 in-flight turn；`core/src/state/turn.rs` 的 map 是 turn 所属，避免拿 provider 字符串直接建立全进程的请求身份。
- 沿用现有 run_approvals，给审批独立的宿主身份，或采用明确编码的 run/turn + call 复合身份；保留原始 provider call_id 关联模型调用，不能为修审批而改坏请求中的 tool call/result 配对。
- 内存 waiter、持久化、RuntimeEvent、Tauri/CLI/SDK 回答 DTO 必须使用同一审批身份，并兼容读取旧数据；覆盖两会话相同 ID、同一 run 的幂等重试、同一调用多次审批及重启后的回答。
- 此缺陷探针默认忽略，生产审批身份及协议未修改。真实重复 ID 频率、桌面表现和迁移策略仍需后续验证。

### H-17：桌面收到取消后的保存失败，却因对话已 settled 而静默丢弃错误

**级别：中（P2）；触发条件：取消已被模型执行循环观察，但取消审计或终态保存失败。** 此项不是取消信号被数据库挡住；停止本身已经生效，缺陷在执行状态与保存状态的 UI 投影。

#### 真实桌面复现

- 重新执行 `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --offline` 成功（约 1m26s），直接启动 Vite 与本次桌面产物，使用独立 `target/live-desktop-test-20261021` 数据目录和现有专用测试供应商配置。通过 CDP 操作实际 Tauri WebView，先验证 `__TAURI_INTERNALS__` 存在，排除普通浏览器 mock。
- 首次探索及随后提交的可重跑探针均复现。探针 `apps/desktop/scripts/audit-cancel-storage.cjs` 不在默认测试/build 中执行；需要 Node 的 `node:sqlite`、Playwright、已启动的隔离桌面以及真实测试模型，会发起一轮付费请求。`AUDIT_DATA_ROOT` 必须解析到本仓库 target/ 的子目录，不能指向用户生产数据。
- 操作：发送不调用工具的长文请求，等真实 `model_first_token`，在隔离 SQLite 中为该 run 安装仅拒绝 `cancel.requested` 插入的 trigger，随后点击“停止”。finally 撤销 trigger 并断开 CDP；不改生产取消代码，不模拟 UI 事件。
- 第二次实测 `cancel_run` 请求到 HTTP 200 约 **2ms**。事件按顺序为 `run_cancelled → session://completed {status:failed,error:"persistence error: audit injected cancel audit storage failure"}`；前者比点击时间晚约 38ms，后者晚约 59ms。这是单次样本，不能声明 P95 达标或所有资源已回收。
- 等待 1.2 秒后的 DOM 和实际截图都无保存失败提示，发送按钮可见、停止按钮消失；`visibleStorageError=false`。探针在“界面必须显示保存错误”的断言明确失败。数据库该 run 保持 `state=finalizing, terminal_kind=NULL, finished_at=NULL`，有 `run_cancelled` 观察事件，但没有成功的 `cancel.requested` 或 `run_terminal`；未把事务回滚误说为终态保存成功。
- 截图与仅含事件元数据的结果：忽略提交的 `target/live-desktop-test-20261021/cancel-storage-failure.png`、`cancel-storage-failure.json`。没有真实制造磁盘写满或损坏生产数据库，故障类型是确定性的审计插入失败。

复跑前启动隔离桌面并配置测试模型，设置 `AUDIT_DATA_ROOT`、`AUDIT_PLAYWRIGHT_MODULE`（本机 Playwright 模块绝对路径）、`AUDIT_CDP_URL` 后执行：

```text
node apps/desktop/scripts/audit-cancel-storage.cjs
```

#### 根因与成熟实现对照

- Kernel 在转发 `RunCancelled` 之后才执行 `persist_terminal()`；保存失败沿 Result 返回。Tauri `start_chat_v2` 正确将这个 Err 转成 `session://completed` 的 failed/error，`api.ts::runChat()` 也正确 reject，错误没有在这两层丢失。
- `App.tsx::finalizeCancelled()` 已将 assistant turn 置为 settled；随后 runChat 的 catch 仅在 `last.turn.phase !== "settled"` 时调用 error finalize。因此合法的迟到保存错误直接被跳过。`onStopRun` 的 cancel IPC catch 处理的是另一条 Promise，无法覆盖这个错误；finally 继续清理 running 状态。
- Codex `tui/src/chatwidget/turn_runtime.rs::on_error()` 先 finalize_turn，再将独立 error event 加入历史并请求重绘；错误展示不依赖 turn 仍处于运行中。它提供可借鉴的“执行结束与错误展示分离”边界。
- 同时核查 Codex `core/src/tasks/mod.rs`：中断标记在 TurnAborted 前尝试 flush，终态后另做 flush；相应 flush 失败在该路径只是 warn。因此不声称 Codex 已通过本系统相同的数据库故障和 UI 验收，也不照搬其日志策略作为完整修复。

#### 影响与修复方向

- 用户无法区分“执行已停止”和“停止结果未保存”，也没有管理/恢复提示；重新打开或重启时可能看到未收尾记录。本次没有据此断言下一次启动一定重复执行工具或恢复成何种终态。
- 在现有运行与通知链路中独立表达执行结果和保存诊断，允许 settled 后追加一次、按 run/session 隔离的保存错误；不能把 cancelled 再改成“执行未停止”，也不能为避免错误而提前谎报 durable terminal。
- 覆盖正常取消与取消错误去重、错误晚于 settled、切换会话后错误、终态保存重试以及重启恢复。若改事件/DTO，沿用同一 run_events/RunStore 边界同步消费方。本轮只增加审计探针和报告，未修生产 UI。

### H-18：输出上限重试后仍截断时，未完成工具调用被补参数并提交

**级别：中（P2），若工具接受空参数并使用有副作用的默认行为，影响可升高；触发条件：首次输出上限恢复已用尽或无法提高，响应仍包含未完成工具调用。** 本项验证到真实 ModelAgent 的决策及 Surface 提交，没有执行真实文件修改或系统命令。

#### 复现与生产路径

- `ChatCompletionAccumulator` 收到 `[DONE]` 会无条件 complete_tool_calls；`finish()` 把空 arguments 改为 `{}`。Responses accumulator 在 `response.incomplete` 后的 `finish()` 也把尚未收到 item.done/arguments.done 的 builder 转成原生 FunctionCall，空参数同样补 `{}`。
- `ModelAgent::think_inner()` 第一次 length 会尝试把输出上限升至 65,536 并 abort 原 attempt，这一步正常。但再次 length 后，只有 `tool_invocations_from_items().is_empty()` 才进入续写恢复；非空时直接 commit，然后在最终 length 错误判定之前返回 CallTool/CallTools，并将原生调用加入历史。
- 默认忽略探针 `model_agent::tests::audit_output_limit_does_not_dispatch_unfinished_tool_calls` 使用生产 ModelClient、两种 accumulator、ModelAgent 与记录型 ToolAttemptController。每种协议分别输入“只开始调用，尚无参数”和“不完整 JSON 参数”，连续两次明确 max-token 终态，另留一份成功响应用于确认是否实际进入恢复。Responses fixture 没有伪造 item.done 或 arguments.done。
- 执行 `cargo test -p deepagent-runtime audit_output_limit_does_not_dispatch_unfinished_tool_calls --offline -- --ignored --nocapture`：四组都仅请求两次，均返回 `Ok(CallTool(...))`，原生 Surface 均保留一个 truncated-call，断言明确失败。Chat 的记录为 `begin:1 → prepare → abort:1 → begin:2 → prepare → commit:2`；Responses 为 `begin:1 → abort:1 → begin:2 → commit:2`。第三份正常响应没有消费。
- 空参数两组决策为 `arguments={}`；半截 JSON 两组带 `__invalid_tool_arguments__` 哨兵。生产 `ToolPipeline::prepare()` 确实会在 hooks/权限/执行之前拦截该哨兵，因此**不声称半截 JSON 能绕过校验执行**。空 `{}` 没有哨兵，后续是否执行仍取决于工具 schema、权限和审批，不等于这些边界被绕过。

#### 官方语义与成熟实现

- [DeepSeek Chat API](https://api-docs.deepseek.com/api/create-chat-completion/) 将 length 与 tool_calls 区分，并要求宿主验证工具参数；length 表示输出达到指定上限，不能仅依据已有工具名字推断调用完整。
- DeepSeek Harness `packages/llm/llm/src/assembler.ts::assembled()` 在 `finish.kind === 'max-tokens'` 时统一去掉 tool-call blocks，并同步过滤 replay metadata；不会把被丢弃的调用在下一请求重放。
- 它的 `llm-pi-ai/tests/loader-composition.spec.ts` 明确用“partial 文本 + 半截工具参数 + length”检查：保留文字，去掉 tool-call，续写请求里没有 tool_calls。这里引用仓库实现/测试作为设计证据，没有在本机执行其完整测试套件。

#### 后果与修复方向

- 未完成调用会成为执行候选、触发一次校验失败/工具循环，并污染原生 replay；空参数还可能改变工具的默认操作范围。此次 fixture 未执行工具，不能将潜在默认副作用说成已发生。
- 在同一模型响应提交边界统一处理终态与调用完整性；截断恢复的保留集合应同时决定决策、原生 Surface、stream attempt commit 及 durable replay，避免仅 UI 隐藏调用。借鉴 Harness 保留可安全续写文本、剔除截断调用并同步 replay 的方式。
- 保留正常完成的无参调用、既有权限/审批和调用配对；新增多调用中途截断、第二次 length、无法升级输出上限、Responses 未闭合 item、取消与截断交错及重启测试。本轮未改生产模型或工具逻辑。

### H-19：子 Agent 的工具限制只过滤 schema，实际执行仍可调用被排除的工具

**级别：高（P1）；范围：ChatSubagentRunner 的内置只读角色、请求 allowlist 和角色 denylist。** 本项实际写入了临时文件，不只是观察模型 schema；没有触碰用户文件，也没有调用真实供应商或系统命令。

#### 触发与复现

- `subagent_runner.rs::run_active()` 克隆父 ToolRegistry；schema callback 通过 `apply_runtime_agent_tool_filter()` 及 request.allowed_tools 过滤模型工具定义，但 RuntimeEngine 接收的仍是完整 sub_registry。`permission_set_for_agent()` 对只读角色也返回 developer 权限，因此工具 schema 限制没有形成执行边界。
- 默认忽略探针 `subagent_runner::tests::audit_read_only_subagent_rejects_tools_hidden_by_its_profile` 使用真实 ChatSubagentRunner、RuntimeEngine、文件工具实现及内存数据库。模型 transport 固定返回一个完整的 write_file 调用，随后正常完成；未安装自动批准 gate。
- 四个独立临时目录分别测试内置 explore、内置 plan、request.allowed_tools=[Read]、自定义角色 disallowedTools=[Write]。逐次检查实际 wire request 均没有 write_file schema；前三组列表分别是只读四工具、只读四工具、仅 read_file，第四组保留其他文件工具但没有 write_file。
- 执行：`cargo test -p deepagent-app-core --lib audit_read_only_subagent_rejects_tools_hidden_by_its_profile --offline -- --ignored --nocapture`。
- 实测四组都 `outcome=Ok, tool_use_count=1`，并在各自目录创建 `audit-child.txt`，内容为 `hidden tool executed`。最终要求零越界写入的断言失败：`[true, true, true, true]`。单组运行约 91–108ms，不是性能统计。
- 当前 WriteFileTool 属于 Low risk，要求 WorkspaceWrite；developer 权限满足该要求。因此本次没有绕过审批程序的证据，缺口是角色/请求声明的工具限制未进入实际 dispatch。测试调用真实子运行，但没有经过完整父会话接纳或桌面 UI，也不声称供应商在自然对话中已返回过清单外工具。

#### 成熟实现对照与修复方向

- Claude Code `tools/AgentTool/runAgent.ts` 先得到 resolvedTools，再组装 allTools 并写入 agentOptions.tools；`services/tools/toolExecution.ts` 从 `toolUseContext.options.tools` 查找实际调用，找不到时返回结构化 `No such tool available`。该实现另有废弃别名兼容分支，不能概括成完全没有 fallback。
- 应在现有 ToolRegistry/ToolPipeline 边界生成同一份有效工具集合，让 schema 与 dispatch 共享其身份和限制；模型返回隐藏工具时应拒绝并补配对失败结果，不能仅用提示词劝模型遵守。
- 角色限制需与父权限取交集，避免只读角色获得更宽的写权限；保留原有文件边界、审批及沙箱校验，不建立第二套工具执行器。覆盖 alias、deferred/MCP 工具、角色 denylist、空 allowlist、后台子运行和恢复后的限制一致性。
- 本轮仅增加审计测试和报告，未修改生产权限或工具路由。

### H-20：DAG 成功节点的工作树被强制删除，文件未交付给主目录或下游

**级别：高（P1）；范围：plan_execute 的 DAG 工作树调度。** 本项用真实临时 Git 仓库复现未提交文件和修改丢失；没有删除本项目或用户工作树。

#### 触发与复现

- `ChatPlanExecutor::execute_plan()` 使用生产 DagScheduler + GitWorktrees。`DagSubagentExecutor` 将子运行重定根到调度器工作树，并指定 shared isolation，因此子运行自己的成功工作树保留逻辑不会接管该目录。
- `DagScheduler::run()` 每个节点执行完后，无条件调用 worktrees.remove，并忽略返回错误；实际 `GitWorktrees::remove()` 执行 `git worktree remove --force`。在这之前没有合并、补丁或资产交付。后继节点从主仓库 HEAD 另建 detached worktree，只收到上游 summary。
- 默认忽略探针 `scheduler::tests::audit_successful_dag_preserves_files_for_handoff` 创建一次性真实 Git 仓库和初始提交，然后使用生产调度器与工作树实现。记录型 executor 的 produce 节点修改 tracked README.md 并创建 untracked generated.txt，现场读回校验；consume 节点确认收到了摘要并检查自己的目录是否有该文件。
- 执行：`cargo test -p deepagent-subagents audit_successful_dag_preserves_files_for_handoff --offline -- --ignored --nocapture`。
- 实测 `all_succeeded=true, producer_retained=false, consumer_saw_file=false, main_has_file=false, main_readme="original"`；要求成功输出至少保留或集成一处的断言失败。上游文件确实写出，却在下游启动前被强制删除；主工作区没有获得 tracked 修改或新文件。
- 此处两节点 executor 为确定性文件 fixture，没有调用模型或完整 ChatPlanExecutor。`all_succeeded` 是 fixture 两次返回 success 的聚合，不代表真实 review 遇到缺文件也必定成功。测试只覆盖未提交修改，不声称已提交 Git 对象不可恢复；没有验证清理错误被吞掉的另一分支。

#### 成熟实现对照与修复方向

- Claude Code `tools/AgentTool/AgentTool.tsx::cleanupWorktreeIfNeeded()` 调用 `utils/worktree.ts::hasWorktreeChanges()`；仅确认无变化时删除，否则返回保留的工作树路径/分支。变化判据包含 dirty tree、新 commits；检查失败也按有变化处理，避免误删。hook 创建的未知 VCS 工作树同样保留。
- 本系统普通 task 子运行成功时已经保留隔离工作树，DAG 所属目录却走了不同的清理语义。应在现有工作树所有权边界统一“产出交付后才清理”，沿用现有子运行记录与结果投影，不新增另一套工作树注册或存储。
- 明确输出交付契约：至少保留可定位、可恢复的工作树/提交；有依赖的下游须得到可读取的产物或受控集成后的基线。仅保留摘要或把路径写进文字，不能让已经删除的文件可用；自动集成涉及冲突和权限，应作为明确策略验证。
- 覆盖修改/无修改、未提交/已提交、失败与取消、合并冲突、清理失败、重启定位和并发 DAG 的工作树身份。此次未改生产清理逻辑或引入自动合并。

### H-21：子 Agent 恢复遗漏原始输出契约，schema、phase、label 全部丢失

**级别：中（P2）；范围：task operation=resume 的原始请求持久化和重新投影。** 本项不是 H-04 的主会话恢复覆盖值问题，而是子运行已有字段在 writer/reader 之间不对称。

#### 触发与复现

- `run_tracked()` 保存 request 时写了 description/prompt/type/allowed_tools/model/effort/skills/isolation，却没有 phase、label、schema；`resume_request()` 明确读取这三个字段，并注释为保留原 frontmatter，但缺失时全部为 None。
- `subagent_system_prompt()` 在 schema 存在时注入原 schema 和必须返回匹配 JSON 的要求。恢复后 schema=None，连同该输出要求一起消失；不是只有管理列表少显示一个标签。
- 默认忽略探针 `subagent_runner::tests::audit_child_resume_preserves_schema_phase_and_label` 通过生产 runner 完成首次子运行、读取真实 SubagentRunStore 和生成的 transcript，然后调用正式 SubagentRunner::resume。两个模型响应是离线 fixture，但两次实际请求都由生产 ModelAgent 构造并记录。
- 执行：`cargo test -p deepagent-app-core --lib audit_child_resume_preserves_schema_phase_and_label --offline -- --ignored --nocapture`。
- 首次实际 wire instructions 包含 audit-contract-phase、audit-contract-label 和 audit_resume_value schema；子运行 succeeded。恢复后仍 succeeded，resume_count=1，但 `restored_phase=None, restored_label=None, restored_schema=None`，恢复的 wire instructions 对应三项均 false；契约相等断言失败。没有伪造 transcript 或手工删除字段。
- 这证明恢复请求不再要求原输出格式，不证明真实模型一定返回无效 JSON。本次未模拟宿主重启，也未验证长历史丢失；原恢复实现另以原任务+2000 字符结果摘要重建请求，完整子历史是否应持久化仍需单独定义和测试。

#### 成熟实现对照与修复方向

- Claude Code `tools/AgentTool/resumeAgent.ts` 并行读取 getAgentTranscript 与 readAgentMetadata，恢复消息/replacement 状态、角色和工作目录后重新运行；`sessionStorage.ts::AgentMetadata` 特别说明缺角色元数据会使 fork 恢复静默退化。Codex `core/src/agent/control/resume.rs` 恢复后返回实际 ThreadConfigSnapshot。可借鉴的是恢复有效状态并验证其完整性，不是照搬它们的字段或存储。
- 本系统应在现有子运行 request/transcript 边界统一序列化与恢复契约，避免 writer/reader 手工字段清单不同步；兼容旧数据，对字段缺失给出明确默认或恢复诊断，不凭空猜出旧 schema。
- 覆盖首次运行→继续、再次继续、跨父 turn、宿主重启、旧 transcript、工具限制和模型配置；完整历史与压缩 replacement 如需恢复，应复用现有事件及 Surface 能力，不能另造子 Agent 运行内核。
- 本轮仅增加测试与报告，并把测试 runner fixture 提取复用；未修生产保存或恢复行为。

### H-22：DAG 子运行只改变声明的工作目录，文件工具仍绑定父工作区

**级别：高（P1）；范围：DagSubagentExecutor 到 ChatSubagentRunner 的工作区绑定。** 本项与 H-20 分开：H-20 是调度器删除已写入工作树的产出；本项是实际工具可能根本没有写入所分配的工作树。

#### 触发与复现

- `DagSubagentExecutor::execute()` 调用 runner.with_root(assigned_worktree)，其实现仅修改 self.root，没有重建 parent registry 中已绑定根目录的工具。请求使用 shared isolation，execution_root() 返回这个新 self.root。
- `run_active()` 仅在 execution_root != self.root 时重绑工具；DAG 路径两者已相等，因此跳过重绑并克隆父工作区 registry。system prompt/checkpoint 的根却是新目录，形成声明与实际读写目标不一致。
- 默认忽略探针 `subagent_runner::tests::audit_dag_child_rebinds_file_tools_to_assigned_worktree` 创建临时 parent 与 assigned-worktree 两个目录，在 parent 上构建真实文件工具 registry；将 assigned-worktree 作为真实 SubAgentContext 传给生产 DagSubagentExecutor。固定模型返回完整 write_file 相对路径调用，之后正常结束。
- 执行：`cargo test -p deepagent-app-core --lib audit_dag_child_rebinds_file_tools_to_assigned_worktree --offline -- --ignored --nocapture`。
- 实测子运行 `ok=true`，每次实际 wire instructions 都声明子目录，但 `parent_output=Some("hidden tool executed"), child_output=None`；要求只写入分配目录的断言失败。真实 WriteFileTool 执行，没有跳过权限或虚构 tool result。
- 本探针验证生产执行边界，用目录模拟调度器已经分配的上下文，没有在该测试再启动 Git、模型网络或完整 plan_execute。与 H-20 的真实 Git 调度器探针互补，但不能将二者拼成已经做过完整端到端模型编排。只验证 write_file，其他绑定工具需独立覆盖。

#### 成熟实现对照与修复方向

- Claude Code `AgentTool.tsx` 将完整同步执行包在 wrapWithCwd/runWithCwdOverride 内；`utils/cwd.ts` 使用 AsyncLocalStorage 为并发子运行派生有效 cwd。FileWriteTool 使用 expandPath/getCwd，路径展开以该执行上下文为依据；提示词和文件执行共享有效目录，不只修改提示文字。
- 本系统已有 run_active() 的 registry 重绑分支，应让工作区绑定与实际 registry 来源保持明确关系，避免 with_root 修改比较基准后跳过重绑。文件、命令、权限、checkpoint、项目查询和模型环境需核对同一有效工作区；保留已有 session_search 会话项目约束，不全局 chdir 或修改共享父 registry。
- 覆盖两个并行子目录写同名文件、父目录不变、相对和绝对路径、sandbox/审批、工具搜索、恢复及普通 task 与 DAG 一致性。H-19、H-20、H-22 显示同一子运行上下文在 schema、工具根、产物所有权处分裂，后续应围绕既有 runner 的有效执行配置统一治理，不逐个增加路径特判。
- 本轮只增加审计证据，没有修生产工具根或删除逻辑。

## 本次验证与交付边界

| 命令或实测 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo test -p deepagent-cli -p deepagent-models -p deepagent-mcp --offline` | CLI 82 passed / 3 ignored；models 84 passed / 1 ignored；MCP 单元 55 passed / 3 ignored、子进程集成 3 passed、协议审查 1 ignored；均无常规失败。组合命令的 feature 合并启用了额外 HTTP 测试 |
| `cargo test -p deepagent-mcp --features http --offline`（新增 H-12 后） | 单元 60 passed / 3 ignored、子进程集成 3 passed、协议审查 2 ignored；无常规失败 |
| `cargo test -p deepagent-cli --offline`（新增 steer 探针后） | 82 passed / 4 ignored，无常规失败 |
| `cargo test -p deepagent-runtime --offline`（新增 H-10 探针后） | 265 单元 + 5 稳定性集成测试 passed，1 ignored；含 500 次以上工具调用测试 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（新增 H-15 后） | 11 passed / 3 ignored，无常规失败 |
| `cargo test -p deepagent-app-core --lib --offline approval_bridge::tests` | 19 passed；常规审批测试未覆盖运行取消后的迟到回答 |
| `cargo test -p deepagent-app-core --lib --offline runtime_payload::tests` | 3 passed / 1 ignored；磁盘锁隔离集成探针另行显式执行通过 |
| `cargo test -p deepagent-runtime --lib --offline tool_pipeline::tests`（新增 H-16 后） | 11 passed / 1 ignored，无常规失败；审批身份缺陷探针另行显式执行复现失败 |
| `cargo test -p deepagent-app-core --lib --offline input_runtime::tests`（新增排队取消测试后） | 28 passed；含取得租约前/后取消及后继输入推进；不代表真实大上下文端到端压缩或完整宿主并发压力已验收 |
| `cargo test -p deepagent-app-core --lib --offline context_runtime::tests`（新增 H-14 后） | 22 passed / 2 ignored；重复内容和无效压缩计费缺陷探针默认忽略，常规测试无失败 |
| `cargo test -p deepagent-app-core --lib --offline run_coordinator::tests`（新增存储故障注入后） | 7 passed；包括审计插入失败后的信号送达、事务回滚、显式重试及唯一终态 |
| `cargo test -p deepagent-cli audit_ --offline -- --ignored --nocapture`（首批执行时） | 当时的 2 个探针明确失败，分别复现 H-01、H-02；后来新增的 resume/steer 探针分别单独执行，见 H-04 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_sandbox_override --offline -- --ignored --nocapture` | 探针明确失败，复现 H-03 |
| H-04 至 H-09 各节列出的单项命令 | 7 个探针分别明确失败（H-04 包含 resume、steer 两个），证据见对应章节；不是常规测试通过 |
| H-10 节单项命令 | 1 个探针明确失败，复现结束后预取仍继续及收尾 usage 缺失 |
| H-11 节单项命令 | 1 个探针明确失败：压缩保留 6 items，数据库恢复得到 54 items |
| H-12 节 http feature 单项命令 | 1 个探针明确失败：真实本机 HTTP 请求丢失 session header，后续 tools/list 收到 400 |
| H-13 节单项命令 | 1 个探针明确失败：已禁用、ready roots 为空，全新会话的实际模型请求仍声明该插件技能 |
| H-14 节单项命令 | 1 个探针明确失败：摘要与主模型均已返回 usage，原上下文保留，但 side_usage 为空 |
| H-15 节单项命令 | 1 个探针明确失败：取消终态后审批仍 Pending，迟到回答受理并产生 tool_session grant；命令执行数为零 |
| H-16 节单项命令 | 1 个探针明确失败：第二 run 的 action 独立登记，但相同 provider id 的审批遭全库幂等冲突 |
| H-17 真实 Tauri 探针 | 当前源码桌面编译通过；真实模型首片段后停止约 2ms ACK；保存失败已送达 UI，但展示断言明确失败 |
| H-18 节单项命令 | 1 个探针四组均复现：Chat/Responses、空参数/半截 JSON 在第二次 length 后仍返回工具决策并保留原生调用 |
| H-19 节单项命令 | 1 个探针四组均复现：wire 无 write_file schema，真实子运行仍成功写入各自临时目录 |
| H-20 节单项命令 | 真实临时 Git 仓库探针失败：上游工作树已删除、下游无产物、主目录仍是初始内容 |
| H-21 节单项命令 | 真实首次运行及 resume 后契约断言失败；恢复计数为 1，但 schema/phase/label 和 wire 指令均丢失 |
| H-22 节单项命令 | 真实 DAG 子执行器及文件工具探针失败；wire 声明子目录，产出实际落到临时父目录 |
| `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`（新增 H-21、H-22 后） | 6 passed / 3 ignored；H-19 提取共享 fixture 后另行显式复跑，四组仍复现同一缺陷 |
| `cargo test -p deepagent-subagents --offline`（新增 H-20 后） | 13 passed / 1 ignored；原有 DAG 测试检查清理和摘要，未校验文件产出保留 |
| `cargo test -p deepagent-memory -p deepagent-knowledge --offline` | memory 40 passed，knowledge 63 passed；均无 ignored 或失败 |
| `cargo test -p deepagent-app-core --lib session_memory::tests --offline` | 31 passed；含空窗口不请求、笔记未改不推进覆盖游标、越界读写拒绝及异步 pass 收尾 |
| `cargo test -p deepagent-app-core --lib memory_service::tests --offline` | 5 passed；vault 写入、主动 recall 与知识检索可见性边界通过 |
| `cargo test -p deepagent-app-core --lib knowledge_service::tests --offline` | 18 passed；项目切换、磁盘重载、draft、检索及离线捕获测试通过 |
| `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`（新增 H-19 后） | 6 passed / 1 ignored；常规角色测试只验证 schema 过滤，没有验证清单外实际调用被拒绝 |
| `cargo test -p deepagent-runtime --lib model_agent::tests --offline`（新增 H-18 后） | 56 passed / 1 failed / 2 ignored；原有 `time_based_microcompact_clears_stale_tool_results_after_idle` 在 `Instant::now() - 3601s` 处溢出，尚未进入生产 microcompact；单独重跑同样失败。本轮未修改该测试或生产时钟，不能报常规测试全绿，也不据此认定生产压缩故障 |
| `cargo test -p deepagent-app-core --lib --offline chat_service::tests`（新增 H-13 后） | 74 passed / 1 ignored；新缺陷探针默认忽略，常规会话服务测试无失败 |
| `git diff --check` | 通过 |

审查累计新增内容限于报告、二十二个默认忽略的 Rust 缺陷探针、一个显式运行的桌面故障探针、两个默认忽略的宿主隔离集成探针，以及默认运行的取消存储故障注入和排队取消测试各一个，未修改生产运行逻辑。本批重新运行了格式检查、CLI/models/MCP/runtime 常规测试及 AppCore 上下文/取消相关测试；没有重复运行全 workspace 测试、Clippy 或前端构建，本批没有改变跨 crate 生产行为或前端代码。桌面使用当前源码真实编译产物测试，截图保存在忽略提交的 `target/live-desktop-test-20261021/`（`tool-result.png`、`restart-recall.png`、`cancel-storage-failure.png`）。

### 后续独立验证：后台 MCP 准备不阻塞聊天

- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e pending_host_mcp_initialization --offline -- --ignored --nocapture`，1 passed。此集成探针需要本机 Node，因此默认忽略；它与 H 系列预期失败的缺陷探针性质不同。
- 180 秒是 fixture 的自然完成时间；本次在两轮对话完成后主动释放，没有实际等待 180 秒。生产 MCP 启动本身还有 30 秒整体上限，测试证明的是“确认握手正在进行时，对话不等待它”，不是承诺握手可运行 180 秒。
- 两轮模型均为离线 replay；真实执行了宿主启动、MCP 进程、ChatService 接纳、组装、Kernel 及模型请求构造。MCP fixture 在释放后接受宽松 legacy 握手，以单独验证生命周期隔离；不覆盖 H-08 的严格协议兼容性。
- 该结果支持当前聊天读取 ready snapshot 的边界；不能由此证明磁盘运行包全量校验也已通过 180 秒延迟注入，更不能证明失败插件的 skills/hooks/说明全部隔离。

### 后续独立验证：运行包磁盘准备等待不阻塞聊天

- 执行：`cargo test -p deepagent-app-core --lib blocked_host_payload_preparation_does_not_block_conversations --offline -- --ignored --nocapture`，1 passed。该探针独占生产全局磁盘准备 gate，应单独运行，默认忽略。
- 创建真实临时 runtime.zip 与插件技能，在启动宿主前对其 `.runtime.lock` 加 OS 排他锁；随后观察生产 `DISK_PREPARATION` 已被 worker 持有，确认它已经进入运行包准备并等待文件锁。没有替换生产准备函数或虚构 Ready 状态。
- 保持锁期间两轮真实 ChatService/Kernel + 离线模型在 349ms 内完成；runtime/value.txt 不存在、ready skill_roots 为空、两次实际请求均没有该技能的完整目录条目。释放后校验/解压完成，技能注册表更新；第三个全新会话的实际请求包含该目录条目，且解压内容与原包相同。
- 首次探针把 fixture 放在聊天工作目录内，单纯匹配技能名会命中动态环境的文件路径；已把工作目录与 fixture 分开，并改为匹配目录条目后重跑通过。该早期断言失败不认定为产品能力泄漏；H-13 也收紧相同判据并独立重跑，缺陷仍存在。
- 这是实际文件锁等待边界，没有真的等待 180 秒，也没有对逐文件校验人为注入 180 秒延迟、统计 P95 或验证跨进程安装并发。它支持磁盘准备在宿主后台执行的隔离设计，不能代替完整运行包性能及损坏恢复验收。

### 后续独立验证：取消审计写入失败不阻止信号

- 新增默认运行的 `run_coordinator::tests::cancellation_survives_audit_write_failure_and_terminal_retry_is_idempotent`，通过真正的 RunCoordinator 接纳并取得运行注册，在内存数据库的 run_events 表安装仅拦截 `cancel.requested` 的 SQLite trigger，使用 `RAISE(ABORT, ...)` 注入写失败。
- 重复 request_cancel 均 accepted；根 token、派生子 token 和 legacy flag 均已取消。调用生产 `kernel::persist_terminal()` 明确返回注入错误；数据库只保留 run_accepted，finished_at 仍为空，说明审计及终态未部分提交，也未把停止落盘说成成功。
- 撤销 trigger 后显式重试，同一取消意图成功写入 `cancel.requested → cancel.propagated → run_terminal`；再次写终态返回 false，最终仅一个 cancelled 终态。释放注册后不再接受该 run 的取消。
- 执行 `cargo test -p deepagent-app-core --lib --offline run_coordinator::tests`：7 passed，其中包括本项故障注入。此项验证已存在实现的正确边界，本轮未修生产代码。
- 边界：没有真实损坏用户数据库，没有实际模拟磁盘写满；显式重试成功不代表所有生产失败都会自动重试。本测试本身不覆盖 UI 或全部模型/进程资源清理；后来真实桌面的错误展示验证见 H-17，结果为缺陷。

### 后续独立验证：排队取消不使后继输入失去租约

- 新增默认测试 `input_runtime::tests::cancelling_queued_input_before_or_after_promotion_does_not_strand_follower`，使用真实 Session 数据库、生产 accept_input_turn、InputLeaseRegistry 与 InputLeaseGuard；固定前驱 owner、待取消输入及 follower 的顺序。
- 分别执行“待取消输入仍在队列”和“前驱已释放、待取消输入刚被提升为 active”两种交错。两者都在有界时间返回取消，待取消项从队列消失；owner 不被错误释放，follower 随后取得租约，最后新输入仍能正常 acquire。
- `cargo test -p deepagent-app-core --lib --offline input_runtime::tests`：28 passed。该测试只验证输入接纳/租约层，前驱的取消回调为记录型 fixture，没有运行并发模型、Tauri UI、完整终态或多进程压力，不能据此覆盖所有会话调度竞态。

后续仍需覆盖逐文件运行包校验长延迟注入、完整宿主并发租约/取消压力、完整压缩的真实 wire 及重启、DeepSeek 流式异常与其他宿主入口。保存失败的 UI 通知现已通过真实故障注入确定为 H-17。此次样本不能用来宣称全系统验收完成，或所有准备与取消问题均已修复。

### 后续独立验证：记忆与知识库已有保护边界

- memory/knowledge 两个 crate 和 AppCore 的 session_memory、memory_service、knowledge_service 测试合计 157 项通过，分别为 40+63+31+5+18。没有因为发现其他模块缺陷而把这部分也定性为失效。
- 核查笔记提取实际使用仅含 pinned note 工具的 registry；这与 H-19 的“完整执行表、仅过滤 schema”不同。原有测试实际验证 off-note 访问拒绝、未编辑不宣称覆盖、放弃的 pass 不保存 watermark。
- 上述是现有离线回归，不覆盖真实模型笔记内容质量、跨进程崩溃瞬间、会话 rewind 后笔记与当前历史一致性，也没有消除前述真实笔记请求次数用例的未定性状态。不能将 157 项通过概括成记忆系统全面验收通过。

## 当前工程收敛方向与实施依赖（审计结论，尚未实施）

下表依据已复现的边界归并；优先级不等于所有问题具有相同触发概率。设计收敛在本系统既有内核，不复制参考项目建立第二套运行链。

| 修复批次 | 关联问题 | 应统一的现有边界 | 首要验收 |
|---|---|---|---|
| 执行权限与子运行上下文 | H-03、H-19、H-22 | 有效工具集合、sandbox 选择、执行根、checkpoint 必须来自同一已解析执行配置；参考 Claude 的有效工具池与子 cwd | 清单外调用不能执行；两个子目录同名写入隔离；声明 sandbox 与实际 backend 一致 |
| 接纳、终态与审批生命周期 | H-02、H-15、H-16、H-17 | Harness 复用 AppCore admission；审批身份带运行作用域；执行停止与保存错误分别投影；参考 Codex 的 turn 所属 waiter 和错误事件 | ACK 后立即取消不发模型；终态后旧审批无效；两 run 同 call_id 独立；保存失败可见 |
| 子运行产物与恢复 | H-20、H-21 | 现有 SubagentRunStore / transcript / 工作树所有权的交付契约；参考 Claude 先检查变化再清理、恢复有效 metadata | 文件可定位并交给下游后才清理；恢复不丢原输出契约；重启后仍能定位 |
| MCP 连接协议与投递 | H-06、H-07、H-08、H-09、H-12 | 现有 MCP manager/transport 内统一握手、会话头、分页、按 request id 分发；连接恢复与业务重试分离 | 严格 Node/HTTP fixture、乱序双请求、模糊成功写调用不自动重放 |
| 模型 Surface、压缩与用量 | H-05、H-10、H-11、H-14、H-18 | 同一原生提交边界决定完整调用、截断续写及回放；预取有 owner；压缩采用与已发生消耗分离；参考 DeepSeek Harness 的 max-tokens 过滤 | 未闭合调用不提交；live/replay 一致；无效摘要仍记已返回 usage；结束后无游离预取 |
| 能力快照与传输恢复 | H-01、H-04、H-13 | 现有快照代次、宿主配置和持久化游标；参考 Codex 的慢连接处理与 Grok 的只读 registry 快照 | 禁用后新请求无残留技能；SDK 感知缺口并可恢复；resume/steer 不丢有效配置 |

其中子运行已经出现三类独立执行上下文分裂（H-19/H-20/H-22）。后续修复应先确定同一配置/工作树/工具注册表的所有权和生命周期，再按这些探针逐项关闭；不能只隐藏工具、修改提示路径或延迟删目录来宣称隔离已成立。
