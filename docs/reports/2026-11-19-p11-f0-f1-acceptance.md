# P11 / F0 / F1+ 验收与协议补齐

日期：2026-11-19。基线提交：`83fdde8e`。验证对象为本轮集成工作树，包含开始前已存在的模型思考、视觉治理和性能改动；本轮不将这些已有改动纳入提交。

## 结论和范围

- P11：通过生产 `ModelClient`、协议工厂和 `ReqwestTransport` 运行 Responses、Messages、Chat Completions 相同的 11 项行为矩阵，最终 **33/33 通过**。另加的跨协议历史矩阵最终 **2/3 通过**；Claude→Responses 因来源请求失败仍未验收。常规矩阵通过不代表全部模型、桌面交互或完整产品门禁均完成。
- F0：重新核对现有 16 个请求方法、通知、ACK、持久化分页、连接退出、steer、终态和审批路由。具体证据见下表。
- F1+：统一所有出站机器事件的版本字段，补 SDK 入站版本校验、TypeScript 缺失事件类型、Rust/TypeScript 的持续增量回放与断线恢复用法，以及 sandbox/status 专项和真实 stdio 覆盖。
- 持续回放采用既有 `thread/read` 的分页查询，追平后间隔 250ms 再读。这是明确的**本地 SDK 设计**，不是服务端推送补发；没有新增 `turn/stream`、存储或运行链。
- 严格格式、Clippy 和 workspace 门禁的结果单独列出，不能用适配层或 SDK 专项通过替代。

## P11：真实接口矩阵

接口使用用户授权的 Layso 地址。凭据只在监督进程内存和子进程环境中传递，没有写入文件或命令参数；生成的报告只保留白名单请求元数据、用量和固定错误诊断，不保存完整请求、图片或模型回复。

入口：`crates/deepagent-models/tests/live_protocol_acceptance.rs`。默认忽略，必须显式提供环境并运行 ignored gate，普通离线测试不会产生远程调用。

| 协议 | 模型 | 实际请求路径 | 测试能力声明 |
|---|---|---|---|
| Responses | `gpt-5.6-sol` | `https://api.layso.ai/v1/responses` | 文本/图片；显式 effort 映射 |
| Messages | `claude-sonnet-4-6` | `https://api.layso.ai/v1/messages` | 文本/图片；adaptive；max 与 high 等价 |
| Chat Completions | `gpt-5.6-sol` | `https://api.layso.ai/v1/chat/completions` | 文本/图片；effort_only |

测试显式设置注册表能力声明，避免独立构造客户端时继承旧 DeepSeek 默认值。它验收声明进入 codec 后的行为，不把能力声明当成模型发现接口的返回值，也不替代自动目录匹配的测试。远程图片识别必须实际回答合成红图的颜色。

最终完整执行耗时 112.04 秒；下表是每项总耗时，包含网络和模型服务，不是 UI 延迟或系统初始化耗时。

| 场景 | Responses | Messages | Chat Completions |
|---|---:|---:|---:|
| 文本及 usage | 通过，3709ms | 通过，3414ms | 通过，3658ms |
| 历史续聊 | 通过，5681ms | 通过，4970ms | 通过，5371ms |
| 自动工具调用及结果续写 | 通过，6559ms | 通过，6745ms | 通过，5814ms |
| 关闭思考 | 通过，3381ms | 通过，1847ms | 通过，2177ms |
| low | 通过，2906ms | 通过，4582ms | 通过，1846ms |
| medium | 通过，2012ms | 通过，2056ms | 通过，1616ms |
| high | 通过，2785ms | 通过，2153ms | 通过，2553ms |
| max | 通过，3749ms | 通过，2279ms | 通过，1783ms |
| 原生图片输入 | 通过，2682ms | 通过，2264ms | 通过，2453ms |
| 请求中取消 | 通过，274ms | 通过，284ms | 通过，288ms |
| 两次相同前缀的缓存用量观察 | 通过，7974ms | 通过，5508ms | 通过，6579ms |

工具为纯合成 echo，没有文件、命令或其他副作用。本项验证模型工具调用和续写序列；实际内核审批、shell 副作用及终态另由 app-server 集成用例覆盖，不把 echo 当成真实权限验收。取消项要求明确返回 cancellation，不能拿任意网络错误计为取消成功。

真实取消样本在请求开始 250ms 后发出取消信号，通常早于首个模型增量，因此证明的是在途请求可取消；收到增量后的活跃流取消另由已有 HTTP 回环/内核测试覆盖，不能混为本次真实模型样本。

### 实际发出的思考参数

所有请求均 `stream: true`，测试输出上限为 1024。

| 用户档位 | Responses | Messages | Chat Completions |
|---|---|---|---|
| off | `reasoning.effort="none"` | `thinking.type="disabled"`，无 effort | `reasoning_effort="none"` |
| low | `reasoning.effort="low"` | `thinking.type="adaptive"`，`output_config.effort="low"` | `reasoning_effort="low"` |
| medium | `reasoning.effort="medium"` | adaptive，effort=`medium` | `reasoning_effort="medium"` |
| high | `reasoning.effort="high"` | adaptive，effort=`high` | `reasoning_effort="high"` |
| max | `reasoning.effort="max"` | adaptive，effort=`high`（本模型声明的等价映射） | `reasoning_effort="max"` |

断言使用独立的预期值，不从生产映射函数反推期望结果。Messages 的 medium/high/max 分别返回 80/136/107 个 reasoning token，均保留 1 个签名；off/low 为 0。Responses/Chat 的这些简单算术样本没有返回 reasoning token。接口没有逐档回显执行强度，因此只能证明请求映射、服务端接受、回复和用量解析，不能仅凭 HTTP 200 证明模型内部计算量严格不同。

### 缓存证据

| 协议 | 最终观察：第一次 命中/输入 token | 第二次 命中/输入 token |
|---|---:|---:|
| Responses | 5120/5275 | 5120/5275 |
| Messages | 0/5641 | 5628/5641 |
| Chat Completions | 5120/5275 | 5120/5275 |

三协议均实际观测到缓存读取。Responses/Chat 在此前轮次已经发送相同合成前缀，因此第一次也可能命中；不能把它描述为冷缓存实验。测试不强制要求正命中，供应商缓存是否保留不是客户端能保证的行为。断点和用量转换继续由已有离线/HTTP 回环测试约束。

### 失败记录及归因

1. 首轮 21 项为 12 通过、9 失败。三项图片在本地因测试缺少图片能力声明而拒绝，尚未发送；补显式声明后由真实模型识图通过。首轮 Chat 使用 `gpt-4.1` 验收思考不合适，且继承了独立 DeepSeek 客户端默认值；最终更换支持该测试能力的模型并提供显式声明。这是测试配置修正，未新增生产特判。
2. 首轮 Responses 指定工具名返回 HTTP 400。独立最小 HTTP 请求同样复现：`tool_choice={"type":"function","name":"acceptance_echo"}` 失败，相同请求改为 `"auto"` 成功并产生调用。最终通用矩阵使用 auto；**指定工具名在该 Layso 模型路由上的兼容性限制仍保留**，没有绕过或伪装成已支持。
3. 第二轮 24 项为 23 通过、1 失败，Chat 普通文本发送报连接错误，耗时 5004ms。未收到参数错误响应，不能据此归因为字段或 codec 缺陷。其余用例继续完成；最终扩大五档后完整重新执行 33 项全部通过。保留此前失败，不能把复跑通过当成网络永不失败的承诺。

这些是验收进程显式发起的不同轮次，没有给生产代码添加自动重试，也没有改变过载立即报错规则。

### 跨协议历史与签名补充验收

独立 ignored test `live_cross_protocol_history_matrix` 在内存中将来源模型的 `ResponseItem` 序列化并反序列化，断言往返相等，再保留用户历史及恢复后的原始 items，交给目标协议续聊。Messages 来源必须实际取得非空签名，否则不能计为通过。

| 路线 | 首轮 | 第二轮 | 最后一次 | 结论 |
|---|---|---|---|---|
| Messages Claude → Responses GPT | 来源 `api_error` | 来源 `api_error` | 来源 `api_error`，4690ms | 三次均只有 `/messages` 请求，没有发送目标 `/responses`；仍缺验收证据，不能归因为目标 codec 或签名拒绝 |
| Messages Claude → Chat GPT | 来源响应体解码失败 | 通过，9291ms | 通过，6905ms | 来源实际有 1 个签名；目标请求与记忆 token 回复正常 |
| Responses GPT → Messages Claude | 通过，7664ms | 通过，6499ms | 通过，6144ms | 来源无签名；目标续聊及 usage 正常 |

首轮为 1/3，后两轮均为 2/3；整个跨协议测试仍返回失败，没有把部分成功标为全绿。每轮均保留脱敏记录，第三次后停止同一路线复验，未增加降级、签名丢弃或供应商特判。

请求字段计数只表示非 null 字段存在，不能充当有效签名证明。特别是 Responses→Messages：来源签名为 0，但目标出现 `signature` 字段；当前 Anthropic SDK 的 `ThinkingBlockParam.signature` 对缺失值默认为空字符串，故不能称为生成了有效 Anthropic 签名。Layso 接受此样本也不能外推为 Anthropic 原生端点接受。

这验证了同一 Layso 服务上的模型/协议历史转换和 item 字节往返，未通过完整 `SessionStore` 写入、origin 恢复及不同真实供应商的组合验收。Claude→Responses 的外部错误需在来源请求能正常完成后再验收。

## F0：方法、生产方、消费方与测试

协议方法唯一声明在 `crates/deepagent-harness-protocol/src/requests.rs`；分发在 `apps/cli/src/app_server.rs`。TypeScript 入口在 `packages/sdk/src/index.ts`，Rust 在 `crates/deepagent-harness-sdk/src/client.rs`，均调用现有方法。

| 方法 | 实现/关键语义 | 实际测试来源 |
|---|---|---|
| initialize | 版本协商；初始化前禁止业务调用 | protocol_contract、版本/初始化契约及真实 stdio |
| thread/start | 保存显式执行配置，复用 Session/EventStore | app_server.rs、configuration_tests、真实 SDK |
| thread/resume | 读取既有会话及配置，不另建运行 | fork/配置/真实重连用例 |
| thread/list | 目录分页、作用域及游标 | catalog_page_tests、Rust/TS 真实目录分页 |
| thread/read | 会话与各 run 独立游标，有界分页 | history_page、cursor_range、terminal_audit、真实重连 |
| thread/fork | 冷启动可访问、历史和文件恢复边界对齐 | fork_tests、terminal_audit、Rust/TS 真实 fork |
| thread/archive | 门禁先于新 run，持久化归档状态 | archive_tests、真实多进程归档 |
| thread/unarchive | 明确恢复，再显式 resume | 同上，重启后状态仍一致 |
| turn/start | 空闲线程准入、ACK 后启动既有 executor | admission/start_exclusivity/terminal_audit |
| turn/interrupt | 现有取消路由；终态由原运行落库 | 取消、EOF、审批等待和真实 SDK 错误回执 |
| turn/steer | 持久化单次替换 claim，再取消原 turn | steering_tests：重复、排队、跨线程、EOF、真实 shell 单次副作用 |
| approval/respond | 统一 ChatService/RunCoordinator 路由 | approval_tests：审批、会话授权、拒绝及诊断 |
| event/ack | 仅当前连接已 flush 的通知，高水位单调 | sequence_tests、并发 outbox、真实 SDK ACK/越界 |
| tool/list | 既有 registry 和已准入能力快照 | tool/list 作用域、schema 哈希、真实 run/重启 |
| config/read | 脱敏的有效配置投影，不读取凭据证明就绪 | config_read_tests、Rust/TS scoped config；三项思考断言仍有冲突，见门禁 |
| sandbox/status | 实际 host 能力和协调器就绪信息 | 新增 sandbox_status_tests + 真实 Rust SDK |

`sandbox/status` 专项同时检查：未初始化拒绝、返回值与 host 能力一致、协调器状态类型正确、不启动 run、不创建线程、不产生 launch。查询状态不等于申请权限或保证某后端可执行。

`connection.rs` 继续使用单一 outbox writer：响应写出后才启动已接受的 worker；ACK 校验与 writer flush 同步。EOF、写失败和背压通过既有取消树结束连接拥有的 turn，清理总预算 10 秒。超时报告未完成清理，不合成成功终态。

`terminal_audit.rs` 及其子模块覆盖真实 executor、持久终态、失败、取消、审批、EOF、背压和 fork。steer 通过原有 continuation claim 保证单次替换，测试实际观察 shell 文件效果一次；没有为 Harness 重建 RunStore、审批或取消中心。

DTO 分离继续成立：机器协议在独立 harness-protocol crate；Tauri DTO 不承担此协议。审批继续复用 `ChatService::resolve_approval_scoped` 到协调器。本轮没有迁移桌面入口或新增网络 daemon。

## F1+：版本与持续回放

### 单一出站版本边界

新增 `HarnessEvent::to_wire_value()`，统一插入当前 `PROTOCOL_VERSION=1`。app-server 通知和 CLI JSONL 均调用它。业务 enum 和历史无版本 DTO 仍可读取，无需逐变体破坏 Rust 构造方式。

Rust/TypeScript 入站接受旧 v1 的无字段事件；对明确的错误版本，包括数字 2、字符串 `"1"`、null，拒绝交付并使连接/调用报协议错误。未知通知方法继续沿用已有兼容规则。TypeScript 补齐 `turn.started` 和 `mcp.lifecycle` 类型。

### 持久增量回放

- Rust：`HarnessClient::watch_thread_events` / `ThreadReplay::next_batch`。
- TypeScript：`DeepAgent.watchThreadEvents(threadId, {checkpoint, signal})`。
- 读取既有 `thread/read`：第一页带 `sessionAfterSequence` 和各 run 的 `runAfterSequences`，续页只传服务器 cursor，耗尽后按新高水位再次读取。
- checkpoint 绑定 thread；每条行验证 stream/run 身份和有序性。会话与 run 的序号分别处理，保留 0；Rust 支持完整 u64，JS 拒绝超出安全整数范围的序号。
- 页解析失败不推进 checkpoint；返回前校验整页。调用方处理完返回批次后再保存 checkpoint，显式重连并使用保存值恢复，不重新提交 turn。
- 一次只保留一页事件；持续等待不会额外缓存全部历史。但 checkpoint 为已见过的每个 run 保存一个高水位，空间随 run 数增长。
- 只交付持久化 session/run 行。actions、approvals、artifacts、graph 是可变快照，仍通过 `readThreadPage` 查询；不伪装成追加事件。
- 不自动 ACK、重新发起运行、重连、取消远端运行或重复审批。TS AbortSignal 和 Rust 丢弃等待只停止本地读取；已发出的读取受原传输 deadline 约束。

这是复用既有分页协议的最小实现，避免新增服务端订阅状态。它**没有承诺 10ms 交付**，也没有完成服务端主动推送的历史追赶。较大历史中每次追平查询仍会遍历有界历史页及相关快照，尚未做大规模 follow 性能基准。

实时文本/思考 delta 是临时投影，持久化可能合并多条文本；恢复的数据不是逐条 live 通知的完全复制。只有持久终态是运行完成依据。

新增测试覆盖独立流、0/u64/JS 精度边界、无序页、非法 cursor、错误线程/行身份、取消本地等待、读取错误不重试。真实 TS CLI 测试在 follow 等待时追加 turn，再关闭进程并用 checkpoint 继续读取；Rust 对实际持久行执行同样的恢复和去重验证。

## 验证命令与门禁

以下为本轮集成工作树上的实际命令与结果。

| 命令 | 结果 |
|---|---|
| `cargo test -p deepagent-models --features http --offline -j 1` | 441 passed / 0 failed / 5 ignored，36 个目标（含 doc tests）；该轮之后新增跨协议 ignored gate，普通离线测试不触发付费请求 |
| `cargo test -p deepagent-harness-protocol -p deepagent-harness-sdk --offline -j 1 --lib --test protocol_contract --test event_version` | protocol_contract 22、event_version 1、SDK lib 88 通过 / 3 忽略；后续最终 SDK 回归另列 |
| `cargo build -p deepagent-cli --offline -j 1` | 通过 |
| `pnpm -C packages/sdk test` | 212 passed / 0 failed |
| `pnpm -C packages/sdk test:stdio` | 6 passed / 0 failed，实际编译的 CLI |
| `pnpm -C packages/sdk test:jsonl` | 1 passed / 0 failed，实际进程与退出回执 |
| `cargo test -p deepagent-cli --bin deepagent app_server --offline -j 1` | 138 passed / 3 failed / 5 ignored，88 filtered out |
| `cargo test -p deepagent-harness-sdk --offline -j 1 --lib --test e2e_stdio` | 最终 lib 88 passed / 0 failed / 3 ignored；真实 e2e 15 passed / 0 failed |
| `cargo test -p deepagent-cli --bin deepagent jsonl --offline -j 1` | 3 passed / 0 failed，231 filtered out |
| `pnpm -C packages/sdk build` | 最终类型构建通过；MCP 事件的 transport/configHash 可选性与 Rust DTO 对齐 |
| `cargo test --workspace --offline --no-fail-fast -j 1` | 已执行完，退出码 101：11 个目标失败，共 60 个失败用例；明细见下表 |
| `cargo clippy -p deepagent-harness-protocol -p deepagent-harness-sdk -p deepagent-models -p deepagent-cli --features deepagent-models/http --all-targets --offline -j 1 -- -D warnings` | 未通过：依赖 `deepagent-persistence/src/document_store.rs:280` 的 `chunks_exact_to_as_chunks` lint；该文件未修改 |
| 上述四 crate Clippy 增加 `--no-deps` | 未通过：已有未提交 `deepagent-models/src/thinking.rs:666` 的 `single_element_loop` lint；本轮未修改该文件 |
| `cargo clippy -p deepagent-harness-protocol -p deepagent-harness-sdk -p deepagent-cli --all-targets --offline --no-deps -j 1 -- -D warnings` | 通过 |
| `cargo clippy -p deepagent-models --features http --test live_protocol_acceptance --offline --no-deps -j 1 -- -D warnings` | 通过 |
| `cargo fmt --all -- --check` | 未通过：未修改 `deepagent-core/src/id.rs` 的 4 处宏排版差异；本轮文件已定向格式化 |
| `git diff --check` | 通过 |

app-server 的三项失败为：`h221_scoped_configuration_is_effective_redacted_and_read_only`、`h222_canonical_thread_defaults_and_turn_overrides_reach_real_wire`、`h222_invalid_thread_effort_is_preserved_and_rejected_before_provider_dispatch`。实际分别为 high 对旧期望 low、high 对旧期望 medium，以及 resolved 对旧期望 invalid。当前工作树已引入新的模型思考映射和兼容别名，这些测试仍按旧直传规则断言。本轮保留它们的失败，没有改生产映射或篡改失败结果；它们仍需在思考能力改造的提交边界统一。

Rust e2e 的原失败是空安装 `config/read.provider` 被旧测试断言为 `deepseek-official`，实际为 null。生产 `run_assembler/execution_config.rs` 在无模型计划时投影 null，TypeScript 真实测试也已有相同断言。本轮只将该项精确改为 null，保留 `uninitialized`、未查凭据和模型为空等约束，未放宽为任意值。

### 完整 workspace 失败明细

以下为 Cargo 最终列出的 11 个失败目标，合计 60 个失败用例。没有重复计算测试内部启动的子进程结果。

| 目标 | 通过/失败/忽略 | 本轮观察到的失败现象 |
|---|---:|---|
| app-core lib | 1744/6/46 | 图像/语音上限和 Hook 等待超时；运行时 Local/Managed 断言冲突 |
| app-core golden_trace | 0/3/0 | 三项均缺少夹具路径（Windows 错误 3） |
| app-core kernel_v2_e2e | 345/28/7 | 缺少夹具路径、未注册模型、思考/输出上限断言、实际命令或 MCP 等待超时 |
| app-core mcp_environment | 0/1/1 | 隔离环境审计宿主失败 |
| app-core runtime_speculation_audit | 9/1/0 | incomplete retry 的 TODO 用例等待超时 |
| builtins lib | 376/3/3 | 三项文件 symlink 夹具因 Windows 错误 1314（客户端没有所需特权）失败 |
| CLI bin | 221/6/7 | 三项思考配置断言冲突；另三项审批/Hook 等待 executor 超时 |
| CLI session_startup_isolation | 2/1/1 | `h89-unknown-thread` 未注册模型 |
| MCP lib | 133/1/1 | 共享 liveness 启动数实际 3、预期 4 |
| MCP env_inheritance_audit | 0/5/1 | 隔离环境审计宿主失败 |
| MCP process_lifecycle_audit | 0/5/0 | PowerShell 执行策略拒绝 `.ps1` 夹具；子进程启动超时 |

CLI 中三项审批/Hook 用例在之前的 app-server 专项运行通过，在完整 workspace 中超时；这只能证明运行结果不同，尚未隔离证实是否为资源竞争。其余失败也没有在清洁 HEAD 重跑，因此不声称全部是既有问题。失败所在代码均未纳入本轮提交，本轮未改系统执行策略、权限或无关测试来强行通过。

本轮没有重建桌面端，也没有运行全部 features 的完整 Clippy 命令；未改 Tauri/桌面 UI，带依赖 Clippy 已被上述 lint 阻断。定向格式检查通过，全仓格式检查的失败保持原样。

## 仍有明确边界

1. 本报告不证明 Layso 指定工具名可用；该路由的独立 HTTP 400 仍需供应商确认。
2. 未运行全部供应商/全部模型组合，也未在本轮重做桌面首启、模型发现新增入库、余额和全部 UI 流程。
3. 没有为真实服务人为制造收费过载、服务端超时或破坏性工具故障；这些行为依靠本地/HTTP 回环及内核测试。
4. 跨协议补充矩阵为 2/3，Claude→Responses 三轮均因来源错误未验收；不同真实供应商及完整会话存储/origin 恢复仍未验收。
5. 当前集成工作树的全仓门禁失败；不能将本轮协议补齐解释为完整方案全部完成。
