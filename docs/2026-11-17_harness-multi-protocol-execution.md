# Harness 多协议开发与验收记录

日期：2026-11-17；更新：2026-11-18。起始提交：`805d5090`。

范围依据：`2026-11-17_harness-multi-protocol-plan.md`、项目 `AGENTS.md` 与当前会话的用户要求。
本记录只覆盖 harness、模型注册表、模型适配及其桌面入口。远程连接由另一会话负责。
开始时工作区已有 34 项未提交变更；逐项保留，不把其他会话的改动纳入提交。

## 用户确认的执行决策

- 过载立即报错，零自动重试，也不切换备用模型。
- “小字”指多余的解释性内容；本轮 UI 去掉冗余说明，保留必要状态和可操作错误。
- 流式能力沿用现有 `turn/start` 后的通知，统一内部契约，不新增独立 `turn/stream` 入口。
- 切换模型时仅在供应商同时改变的情况下重新查询余额。
- 模型切换从下一轮用户消息生效；当前 turn 的工具调用与续写始终使用 admission 冻结的模型。
- 所有模型默认有思考能力；未声明时开放五档，最低档表示关闭。此前“手工配置前全部置灰”作废。
- 每轮先运行失败测试，再修复并运行回归；记录真实结果，不能改写验收标准或绕过步骤。
- 测试凭据仅用于用户授权的接口验收，不进入代码、命令输出、截图明文或本记录。

## 最终验收门槛

以下项目必须各有实际证据后才能宣告完成，局部通过不等于整体通过。

| 项目 | 要求 | 当前证据 |
|---|---|---|
| 首次登录 | 新数据目录填写 DeepSeek 密钥后进入会话并真实对话 | 待验收 |
| 供应商管理 | UI 添加 Responses 与 Messages 配置、拉取并激活模型 | 待验收 |
| 会话选择 | 按供应商显示激活模型，实际请求使用选中的地址、协议、模型和密钥 | 待验收 |
| 切换与余额 | 当前输出不中断；后续请求使用新选择；仅换供应商时重新查余额 | 待验收 |
| 中性事件与持久化 | signature/origin 往返、旧记录兼容、跨供应商历史回放 | 待验收 |
| 协议适配 | SDK 类型与序列化接入，HTTP/SSE/取消/预算复用既有边界 | 待验收 |
| 双协议行为 | 相同文本、思考、工具、usage、失败、取消等行为用例覆盖两协议 | 待验收 |
| 缓存与预算 | 稳定前缀、Messages 缓存断点、模型窗口与输出上限接入既有压缩策略 | 待验收 |
| UI 与性能 | 实际 UI 操作验证，记录切换、加载、错误反馈；无冗余解释文案 | 待验收 |
| 工程门禁 | 相关与 workspace 测试、fmt、clippy、桌面构建 | 待完整验收 |
| F 批次 | 补齐既有协议与 SDK 的真实缺口，统一审批、取消与回放 | 待验收 |

## H001：保留迁移供应商的密钥引用（P3）

根因：V24 导入行引用 `deepseek_api_key`，服务保存、轮换与删除却重新拼接
`harness_provider:<id>`。编辑元数据会切断原引用，轮换会复制到另一名称，删除会遗漏旧密钥。

RED：先新增 3 个回归用例，使用 V24 真实导入行的字段形状和内存 SecretStore，
分别覆盖元数据编辑（空值与空白）、密钥轮换、删除且保留无关供应商密钥。

命令：`cargo test -q -p deepagent-app-core --test harness_model_service --offline -j 1 legacy_provider`。
结果：3 failed / 0 passed；分别复现引用丢失、引用名被改写、原密钥未删除。

修复：已有供应商始终使用其持久化 `api_key_ref`；新供应商才生成名称。
查询存量行复用服务内的单一辅助函数；不创建第二套密钥存储或特殊 DeepSeek 分支。

GREEN：`cargo test -q -p deepagent-app-core --test harness_model_service --offline -j 1`，
11 passed / 0 failed / 0 ignored。修改文件经 rustfmt 格式化，提交前检查相关 diff。
本提交只包含服务、其回归测试和本执行记录。

## H002：过载零重试与移除备用模型（P3）

RED：`cargo test -p deepagent-runtime --offline -j 1 --lib overload_fails_without_retry_with_the_default_attempt_budget`，
0 passed / 1 failed。未指定 attempts=1，真实复现过载后的第二次请求返回成功。
`cargo test -p deepagent-models --lib failure::tests --offline -j 1`，1 passed / 2 failed，
复现统一分类仍把过载标记为可重试。

修复：统一失败分类中的 Overloaded 不再可重试；显式过载的 HTTP 503 也保持过载分类。
承接此前未提交的备用模型删除，保留既有取消、权限、工具与调用计费边界。
同模型重试场景使用实际可重试的连接/服务故障，过载场景验证一次调用、工具 abort、usage 与失败终态。
用量测试同时检查按模型合并的既有投影和按物理调用记录的 CallUsageSink，避免把两者混淆。

GREEN（均 0 failed）：models lib 240 passed；runtime lib 493 passed / 2 ignored；
H55 双旧协议全部 effort/取消/过载适配 4 passed；AppCore H144 7 passed；
settings 97 passed / 2 ignored；CLI `retry_producer` 2 passed。
真实 Reqwest/TCP 过载（529 与明确 503，携带 Retry-After）1 passed，验证单次 HTTP 调用且不等待重试。
内核串行采样 1、计费/预算 5、输出上限 1、图像模式 4、模型快照/重开 2 项全部通过；
启用 `web,runtimes` 的过载/图像回归另有 3 passed。
全仓内核并行跑法发生多项超时，本会话停止该次运行后以上串行相关回归通过；
取消的那次整组运行不计为通过。完整 workspace 验收仍待后续工程门禁。

格式：修改的 Rust 文件已 rustfmt；`git diff --check` 通过。
全仓 `cargo fmt --all -- --check` 发现既有 `deepagent-core/src/id.rs` 的四处宏排版差异，尚未处理。

环境：另一会话占用默认 Cargo 缓存，本轮使用临时缓存副本并保持 `-j 1`。
桌面构建要求的 Node 24.21.0 已按项目 runtime_service.rs 的固定 SHA-256 校验安装到托管目录，
未改构建脚本或降低版本门禁。首次完整桌面构建在插件来源指纹检查处失败，见 E001。

## E001：修复 Windows 检出导致的插件指纹不稳定

RED：完整 `pnpm build` 在 `check-plugin-licenses` 失败；`wedecode` 实际目录指纹
`96a656ad...` 与登记的 `7a1c4339...` 不一致。

查证：逐文件比较 Git HEAD 的 5 个 blob 与工作目录；4 个文本文件只有 CRLF/LF 差异，
`runtime.zip` 字节完全相同，无额外文件。历史提交 `c568f977` 曾仅更新登记值以解除同类阻塞，
但未约束 Windows 检出的换行；现有 `.gitattributes` 已为 REA 和 Superpowers 固定 LF，遗漏 wedecode。
4 个文本文件的所有 LF/CRLF 组合均不能复现旧登记值，说明登记值本身也已偏离受版本控制的源码。

修复：沿用已有 `.gitattributes` 规则，固定 wedecode 文本 LF、ZIP 二进制。
仅将确认与 HEAD 内容一致的文本恢复为规范换行；将清单与 notices 登记为规范 Git 源码的
`sha256:a1552075c3f819c35653e9abd230009945fd1dd55ca01b0a5c67b26e5551fff4`。
未改插件源码、版本、许可证、运行包或哈希检查算法。

GREEN：`node apps/desktop/scripts/check-plugin-licenses.cjs`，10 个插件全部通过；
`node --test apps/desktop/scripts/check-plugin-license-hash.test.cjs`，1 passed。
完整 `pnpm build` 已通过（包括前置脚本、tsc 和 Vite）；本项不等于 UI 或产品功能验收通过。

## H003：首次登录与旧 Chat 模型的一次性种子迁移（P3）

RED：`cargo test -q -p deepagent-app-core --test harness_registry_seed --offline -j 1`，
4 failed / 1 passed，复现登录和旧配置未激活模型、迁移写入缺少事务边界等问题。

修复：首次登录发布和旧配置读取共用一次性种子迁移，只导入旧 Chat 选择。
供应商、模型和迁移标记在同一 SQLite 事务提交；密钥沿用 `deepseek_api_key` 引用。
已有编辑与停用状态优先，迁移完成后删除供应商不会在冷重开时复活。
登录迁移失败沿用既有凭据 Committing 状态，显式重新登录成功后才恢复可用。

GREEN：种子迁移最终 7 passed，覆盖容量/模态保留、无配置、重复加载、删除后冷重开、
写入失败原子回滚、凭据失败关闭与恢复、已有用户编辑保护。
`cargo test -q -p deepagent-persistence --offline -j 1` 全部通过：221 + 9 + 5 项，0 failed。
settings lib 97 passed / 2 ignored。修改文件已 rustfmt；提交前检查 diff。
本批只完成注册表初始化，生产运行链仍待 H004 接入。

## H004：注册表接入真实运行链与冻结模型（P3，2026-11-18）

RED：新增 `harness_registry_run` 先取得 0 passed / 5 failed：注册表中非旧 catalog
模型被拒绝、停用旧模型仍可请求、快照缺少真实 provider、未绑定对应供应商密钥。
随后技能审查/标题回归 0 passed / 1 failed，复现辅助调用仍走旧固定地址。

修复：数据库用一个 JOIN 读取选中供应商和模型；统一 resolver 校验激活、协议与能力，
模型身份和容量以注册表列为准。admission 冻结地址、协议、模型、能力、版本及密钥引用，
执行时精确绑定该引用，当前 turn 的工具续写复用同一 client；下一轮才解析新选择。
迁移密钥继续经过既有凭据生命周期 gate。能力 JSON 损坏会失败，错误不回显原文。
技能审查与标题也使用同一 resolver，保留原有输出上限、预算和计费边界。
旧请求 provider alias `deepseek-official` 兼容映射到 `deepseek`；机器快照报告真实注册表身份。
未初始化快照的 provider/protocol 等路由字段为 null，SDK 类型与说明同步。

测试夹具按新契约明确预置并激活其需要的模型；生产不会因发现模型就全部激活。
CLI 原配置回归先取得 8 passed / 1 failed，修正夹具后配置组 15 passed。
真实 SDK/CLI 先复现旧 provider banner 断言失败，更新为空路由的真实语义；
所有真实 CLI 测试统一支持既有 `DEEPAGENT_TEST_BINARY` 选择，使用本轮新构建及字节哈希，未跳过。

GREEN（均 0 failed）：
- `cargo test -p deepagent-app-core --test harness_model_resolver`：11 passed。
- `cargo test -p deepagent-app-core --test harness_registry_run`：7 passed。
- 注册表服务 11 passed；种子迁移 7 passed；settings lib 102 passed / 2 ignored。
- 内核串行：model_selection 8、output_limit 1、reasoning_capability 2、session_title_usage 16、
  credential_lifecycle 1、sampling 1，共 29 passed；model_runtime lib 另有 2 passed。
- persistence 全部 235 passed；CLI configuration_tests 15 passed；CLI build 通过。
- SDK `pnpm test`：200 passed；`test:stdio` 6 passed；`test:jsonl` 1 passed；含 TypeScript 构建。
- `web,runtimes` 的 `h66_tracked_child`：1 passed；首次链接触及 MSVC PDB LIMIT (12)，
  仅通过本次命令的 `profile.test.package.deepagent-app-core.debug=1` 降低调试符号后通过，
  未改功能、测试、仓库 profile 或断言。

格式与工程门禁：相关修改已 rustfmt，`git diff --check` 通过。
全仓 fmt 仍受既有 `deepagent-core/src/id.rs` 四处宏排版阻塞。
全仓 all-features clippy 已实际执行，离线依赖缺少 `alloc-no-stdlib v2.0.4`，未通过；
随后 `cargo fetch --locked` 已补齐锁定依赖，未改 Cargo.lock，clippy 等待重跑。
完整 workspace 测试与后续工程门禁仍待执行，不能据局部回归宣告全部完成。

边界：Messages codec 尚不存在，选择 Messages 仍明确失败。默认模型暂从旧 Chat 字段取值，
新默认/会话选择、辅助调用与父 turn 的显式供应商选择绑定待 P10；独立视觉路径待后续删除。
尚未做本批真实外部 API、桌面 UI 或性能验收。

## H005：会话模型来源持久化（P3b，2026-11-18）

目标：让每条模型产生的中性 `ResponseItemAppended` 同时保留供应商、模型和协议来源，
不新增正文表，不改变现有 zstd JSONL、projection 或回放边界；旧记录缺少新字段时继续可读。

修复：`ModelClient` 保存当前运行来源，真实注册表运行链在 `ModelAgent` 输出落盘时把来源传给
现有 `Session` 事件；非模型事件和旧辅助构造保持 `origin: None`。`ModelOrigin` 只包含
`provider_id`、`model_name`、`protocol`，不写 endpoint 或密钥。模型来源与当前 turn 的冻结 client
一致，工具续写不会重新读取注册表。

GREEN（本批相关回归）：
- `cargo test -p deepagent-core --test harness_model_origin --offline`：2 passed。
- `cargo test -p deepagent-persistence --test harness_model_origin --offline`：1 passed。
- `cargo test -p deepagent-persistence -p deepagent-session --offline`：persistence 221 + 1 + 9 + 5，
  session 27，全部通过。
- `cargo test -p deepagent-app-core --test harness_registry_run --offline`：7 passed，包含真实运行
  事件来源及跨用户轮次模型切换断言。
- `cargo test -p deepagent-models -p deepagent-runtime --lib --offline --jobs 1`：models 240、
  runtime 493 passed，runtime 2 ignored。
- AppCore 单独复跑的媒体边界测试通过；workspace 低并发回归的 AppCore lib 为 1718 passed、
  44 ignored、2 failed。失败已单独复现：`max_reasoning_keeps_chat_model_without_inventing_output_limit`
  仍断言不发送 `max_output_tokens`，与注册表输出上限接入后的行为冲突；
  `project_runtime_wins_over_managed_fallback` 在当前机器 PATH 的 Node 24 满足条件时得到 `Local`，
  而测试仍断言 `Managed`。两项均不是本批来源字段改动，workspace 门禁不能宣告通过。
- `cargo clippy --all-targets --all-features --offline --jobs 1 -- -D warnings` 未通过：已有
  `crates/deepagent-persistence/src/document_store.rs:280` 触发
  `clippy::chunks_exact_to_as_chunks`；未修改该无关问题。

边界：跨供应商回放时 `Thinking.signature` 是否需要降级为普通文本仍待真实 provider 实测；
本批只保留签名和来源，不擅自改变协议语义。Messages codec、余额按供应商分发、会话选择器、
桌面 UI 流程和真实外部 API 验收仍未完成。

## H006：Messages codec 与 SDK DTO 接入（P4/P5，2026-11-18）

RED：新增 Messages 请求、流式生命周期、thinking/signature、工具参数、usage、错误、取消和截断
夹具；此前 `wire_mode_for_protocol("messages")` 明确返回无 codec 错误。

查证：`借鉴/anthropic-sdk-rs` 固定提交 `16dc3e424e7c5041bcc952ebb54d9ec4b2b1ced7`，SDK 仅
承担类型与序列化参考；其网络客户端使用 reqwest 0.13，不能注入本仓库 reqwest 0.12 的
`HttpTransport`，因此 HTTP、SSE、取消、预算和错误边界继续由本仓库负责。该提交为 MIT 许可。

修复：新增 `WireMode::Messages`、Messages 请求投影和流式 accumulator；供应商系统提示放入顶层
`system`，工具 schema 映射为 `input_schema`，`tool_use`/`tool_result` 保持中性工具调用顺序，
thinking 的签名和 `output_tokens_details.thinking_tokens` 保留；响应通过现有 `Response`、
`DeltaObserver` 和模型来源事件进入运行链。Messages 图片、主机 discovery item 和工具结果图片
当前显式报错，避免静默丢失数据。

GREEN：
- `cargo test -q -p deepagent-models --test messages_client --test messages_stream --test messages_usage --offline -j 1`：1 + 10 + 1 passed。
- `cargo test -q -p deepagent-app-core --test harness_model_resolver --test harness_registry_run --offline -j 1`：11 + 7 passed。
- `git diff --check`：通过。

本批仍未完成余额按供应商分发、五档思考 UI/真实请求、会话模型选择器和真实外部 API/UI 验收；
Messages 的图片与缓存断点也需在后续 provider 验收中补齐。完整 workspace 测试、fmt 和 clippy
仍需按工程门禁复跑。

## H007：余额适配、Composer 五档思考与模型切换（P7/P9/P10，2026-11-18）

RED：现有余额入口固定调用 DeepSeek 官方接口；Composer 只读旧 settings 的模型列表和推理档位，
不会消费 harness 激活模型，也不能按供应商切换或在切换后刷新余额。

修复：新增 `BalanceKind` 的 `none/auto/official/new_api/sub2api` 分发，`Auto` 按 new_api → sub2api
探测并统一余额结果，错误中不回显密钥。Composer 改为消费激活 harness 模型，按供应商分组；
思考档位收敛为 `off/low/medium/high/max`，按模型能力置灰；模型选择持久化到 harness 注册表，
跨供应商切换触发余额刷新。当前 turn 继续使用冻结的模型和 client，工具续写保持旧模型，下一轮用户消息
才重新 admission。为兼容现有 Rust `ReasoningEffort` 接口，`medium` 暂映射为 `high`。

GREEN（均 0 failed）：
- `cargo test -q -p deepagent-models --lib --offline -j 1`：249 passed，包含余额适配、Messages 和既有模型回归。
- `cargo test -q -p deepagent-models --test messages_client --test messages_stream --test messages_usage --offline -j 1`：1 + 10 + 1 passed。
- `cargo test -q -p deepagent-app-core --test harness_model_resolver --test harness_model_service --test harness_registry_run --offline -j 1`：11 + 12 + 7 passed。
- `cargo test -q -p deepagent-persistence --offline -j 1`：222 + 1 + 9 + 5 passed。
- `cd apps/desktop && pnpm build`：前置脚本、TypeScript、Vite 构建全部通过；Composer 专项 31 项通过。

边界：P8 缓存治理仍未实现；真实余额 provider API、完整模型设置页和 Composer 的桌面手动验收仍未完成。
全仓 fmt 仍受既有 `deepagent-core/src/id.rs` 排版差异影响；全仓 clippy 仍受既有
`deepagent-persistence/src/document_store.rs:280` 的 `chunks_exact_to_as_chunks` 影响。

本轮状态复核补充（静态核对，尚未修复，不能计为验收完成）：
- P7：`BalanceChip` 与 `SessionUsageBar` 捕获所有 harness 余额错误后回退旧 DeepSeek 入口，
  可能掩盖当前供应商故障；会话用量栏仍将任意余额单位当人民币格式化，无限额度字符串也进入数值解析。
- P7：new_api 在缺少 limit 时默认 `unlimited = true`，未知响应形状可能被当作无限额度，
  与方案“未知形状显式失败”不一致；已有 fixture 不覆盖此场景。
- P9：`medium` 写入后变为 `high`；能力投影读取 `/effort/supported_levels`，
  尚未落实方案的 `thinking_levels` / `thinking_distinct`。UI 在 `thinking_disable = true` 时禁用 off，
  与方案 §1.4 中 false 表示不能关闭的语义相反。
- P5：工具结果校验目前只检查数量和已知 ID；两个 tool_use 对应两个同 ID 的 tool_result 时，
  仍可能通过校验而遗漏另一调用。已有顺序用例和未知 ID 用例不覆盖此重复场景。
- P5：现有 `TransportRequest` 仅携带 URL、Bearer Key 与 JSON body，`ReqwestTransport` 尚未构造
  Messages 专用请求头。仓库 SDK 的 `client.rs` 会构造 `anthropic-version` / `x-api-key`；
  当前 mock 用例只验证 body/endpoint，不能据此认定官方 Messages HTTP 接入已经可用。

workspace 首次复跑在编译 CLI 时发现两处 `WireMode::Messages` 非穷举匹配；
已在旧双协议 retry producer 的测试夹具中显式拒绝未实现的 Messages fixture，保留其原有双协议测试范围。
再次完整执行 `cargo test --workspace --offline -j 1`，在
`crates/deepagent-runtime/tests/h55_thinking_adapter.rs:45`、`:75`、`:112` 的三处匹配编译失败，
均为新增 `WireMode::Messages` 未覆盖。此项为本轮协议扩展的消费方迁移遗漏，尚未修复；
不能算作历史环境问题，也未取得本轮 workspace 测试通过结论。

## H008：Messages 请求契约与第三协议运行回归（2026-11-18）

RED：新增三条真实失败输入，取得 3 passed / 3 failed：工具调用 a/b 的结果为 a/a 时漏验，
中性 `required` 被原样写成 Messages 不支持的类型，关闭思考被写成 `output_config.effort=none`。

修复：请求外层使用已固定版本的 SDK `MessageCreateParams`、`ToolChoice`、`ThinkingConfig` 和
`OutputConfig`，由 SDK 负责字段序列化；中性 required 映射 Any，关闭使用 Disabled，开启使用
Adaptive 加 effort。工具结果同时校验唯一 ID、集合和顺序，重复结果在发请求前失败。
H55 补齐 Messages endpoint、真实生命周期 fixture、输出上限和请求断言；同组用例实际覆盖三协议的
429 重试、503/529 过载零重试、思考默认/显式关闭以及调用前/调用中取消，没有 wildcard 跳过。

查证纠正：DeepSeek 官方 `guides/anthropic_api` 明确支持 Bearer 或 x-api-key，忽略
anthropic-version，adaptive 视为 enabled，支持 disabled。H007 关于缺少专用头的记录是
原生 Anthropic 兼容性待办，不能据此判定本轮提供的 DeepSeek Messages 接口不可用。
原生 Anthropic 请求头与真实 HTTP 验收仍需后续完成。

GREEN：Messages 请求 6 passed；`cargo test -q -p deepagent-runtime --test h55_thinking_adapter
--offline -j 1` 为 4 passed；models lib 完整回归 256 passed，Messages 集成 1 + 10 + 1 passed。

## H009：余额语义与供应商查询边界（2026-11-18）

RED：余额组 8 passed / 6 failed，复现空余额伪造零、未知 new_api 响应误判无限、缺用量时把额度
当余额、total_usage 的分币单位错误、token display 金额/单位丢失，以及 DeepSeek Messages base
错误拼接余额 endpoint。前端新增余额回归最初 0 passed / 7 failed。

依据：`借鉴/cockpit-tools/.../codex_model_provider_commands.rs:1349-1409`：total_usage 除 100，
token data.display 金额与单位整体使用，不与 billing 混用，不猜原始 token 额度换算倍数。
本地设计：内部金额运算使用 checked micros；DTO 保留现有可选 amount，无金额/可用性继续为未知。

修复：无限须显式标志或已核实的三个 sentinel 值；未知形状失败。缺 limit/used 时不计算余额。
DeepSeek 官方账号余额固定使用同 host 的 /user/balance，兼容模型 base 的 /anthropic 路径。
前端两处余额共用 getBalance，选中供应商失败直接呈现失败，不再查询另一个供应商；只在没有
注册表选择时兼容旧入口。统一按货币/token/无限格式显示，未知不补零；快速切换的迟到结果丢弃。

GREEN：models lib 256 passed，Messages 集成 12 passed；余额前端专项 9 passed（包括两处组件的
迟到结果行为）。复跑 `pnpm test:session-usage` 为 25 passed（含余额专项），
`pnpm test:number-format` 为 3 passed。完整前端构建和 AppCore resolver/service/registry_run
专项通过，后者分别为 11 + 12 + 7 passed。
全仓 fmt 仍仅报 deepagent-core/src/id.rs 既有四处格式差异；git diff --check 通过。

本批尚无真实外部 API、完整首次登录/设置页/Composer 流程或性能验收证据。
P4 Responses SDK、P8 缓存、P9 五档端到端、旧视觉路径删除、P11 与 F1+ 均未完成。

## H010：统一三协议 HTTP 回归夹具（2026-11-18）

RED：启用 http 的模型 all-targets clippy 在 `sse_http_framing.rs` 因新增 Messages 枚举
未穷举而编译失败。全局检查另外找到 UTF-8、生命周期、内部 HTTP 审计和运行时测试的同类遗漏。

根因：协议枚举扩展后，公共传输测试各自维护协议矩阵及生命周期，消费方迁移没有统一边界。
本地设计：新增一个仅供测试的 `sse_protocol_fixture.rs`，集中三协议矩阵、endpoint、
文本 start/delta/finish 和 SSE 编码，由模型 HTTP 测试、内部 UTF-8 审计及运行时测试共用。
生产 HTTP、取消、预算与持久化没有新增第二条链路。

扩展 UTF-8 逐字节、绝对偏移、CR/LF/CRLF、多行 data、末尾空行、活跃流、idle 和真实取消
用例到三协议。Messages 输入用量在 message_start 已发生，因此错误断言保留该真实用量，
只禁止伪造输出用量或完成事件；不套用旧协议“任何 usage 都不能出现”的断言。
运行时思考与截断工具测试使用完整 Messages 生命周期，不通过 wildcard 跳过新协议。

开发环境：`pnpm dev` 曾在 Windows 监听 `src-tauri/resources/skills/**` 时抛出 UNKNOWN watch。
依据 `借鉴/cockpit-tools/vite.config.ts:82-84` 和当前 Vite WatchOptions，排除 Rust 资源及
target 目录。复跑 dev 已启动（Vite ready 33145ms），浏览器看到首次登录页；尚不代表原生桌面
登录和 API 对话验收通过。未操作或提交工作区中既有的资源文件删除。

GREEN：
- HTTP SSE framing / UTF-8 / lifecycle：2 + 5 + 3 passed，覆盖三协议。
- `cargo test -q -p deepagent-models --features http --lib --offline -j 1`：279 passed。
- `cargo test -q -p deepagent-runtime --lib --offline -j 1`：493 passed / 2 ignored；
  截断工具审计必须在第三次完整响应恢复为 recovered，共调用三次，已实际通过。
- `cargo check --workspace --all-targets --all-features --offline -j 1`：通过，耗时 15m05s。
  首次因 installer/dist 不存在失败；先运行安装器真实前端构建（通过）再执行检查，没有伪造产物。
- models/http/all-targets clippy：首次被既有测试的 manual_is_multiple_of lint 阻塞；
  按当前工具链建议修改该一处测试表达式后复跑通过。
- 桌面 `pnpm build`（含前置测试、tsc、Vite）通过；`git diff --check` 通过。

这不代表完整 workspace 测试或全仓 fmt/clippy 通过；既有 AppCore 两项失败与全仓门禁问题仍待处理。

## H011：补齐 TypeScript SDK 的既有方法入口（F1，2026-11-18）

依据 F0 缺口 3、Rust requests/results 类型及 Rust SDK 的现有方法；不改服务端方法、协议版本、
字段或权限。RED：三个公开入口用例为 0 passed / 3 failed，均因方法不存在而失败。

修复：TypeScript SDK 增加 `forkThread`、`sandboxStatus`、`acknowledgeEvents`；复用既有
stdio request、版本握手、错误与回执验证。fork 返回完整 receipt，不自动 resume 或重试。
fork/ACK 的序号在发包前校验安全无符号整数，避免 JavaScript 精度丢失；不把 ACK 解释成持久游标。
公开入口负例验证错误回执、原始 RPC 拒绝和非法序号零发包。原有真实 CLI fork 用例改走公开方法。

验证：SDK TypeScript 构建通过。默认并行全测 201 passed / 2 failed；两个失败为 H266/H267
子进程生命周期用例的 gate/时序超时。使用相同测试文件与断言、仅 `--test-concurrency=1`
复跑 203 passed / 0 failed。随后新增回执负例和省略 atSequence 断言，相关两文件 38 passed。
最终相同文件集合串行全测为 204 passed / 0 failed。真实 CLI cold fork 用例 1 passed，
包含 4 个原生进程、3 个分支、4 次显式运行，使用公开 forkThread 方法；既有 CLI 二进制 SHA256 为
`fcaa80efa5a49a8fece86091cd695a45e320f39460f74abfc8aeadc229e1983f`，本批未重新构建 CLI。
默认并行失败保留为工程限制，不能记为默认全测通过。

H018 后续按 package.json 的原始 pnpm test 命令复跑：默认并行 204 passed / 0 failed，
耗时 7594ms。本次通过不抹去此前两项超时证据；未为通过修改脚本并发或测试超时。

## H012：统一默认思考能力与 medium 全链路（2026-11-18，验证中）

用户纠正优先于此前回答：缺省能力采用五档且可关闭，保留显式配置。
用户随后说明“预设”指方案设计，不要求新安装默认选中关闭；保持初始高档和已保存的选择。
根因：UI 读取旧 effort 元数据、off 禁用条件相反；前后端把 medium 转成 high；
Chat Completions 的直接及桥接入口没有消费注册表思考能力。

RED：能力服务原用例 12 passed / 3 failed；三协议 H55 为 3 passed / 1 failed；
前端 medium 保存曾得到 high。新增真实 registry → ModelClient → transport 矩阵为
0 passed / 1 failed，受限档位在 Chat 入口仍实际发包；最低档中文标签断言为 13 passed / 1 failed。

修复：中性能力解析由模型层共享，UI 与 admission 同源；能力缺省开放五档，显式空表保持空，
非法类型、未知档位及重复档位报错且不回显值。medium 成为独立 canonical 值，同步设置、
斜杠命令、子代理工具 schema、Tauri/TS 契约和真实请求断言；旧 thinking_depth 迁移保持原语义。
选择器最低档显示“关闭”，全禁用滑杆拒绝指针与键盘提交。

补充兼容 RED：旧 effort 声明只支持 low 时，高档仍能实际发包，真实三协议矩阵为
0 passed / 1 failed。共享解析补上旧 effort 限制（新 thinking_levels 优先），非法旧声明仍拒绝。

补充 UI RED：激活列表为空仍回填旧模型，未选择模型时擅自显示列表首项，15 passed / 2 failed。
成功读取注册表后以该结果为准，空结果不回填旧模型；未选择时不伪造首项选择。最终 17 passed。

已验证：前端 controls 17 passed（含各保存档位不触发关闭写入）；能力解析单测 4 passed；
能力服务 15、resolver 12（含真实三协议发包矩阵与旧声明限制）、
registry run 7 passed。桌面完整 pnpm build（前置测试、TypeScript、Vite）通过。
全仓 fmt check 目前仅剩 core/id.rs 四处既有排版差异；本轮修改文件已格式化。

使用用户授权的两组目标进行直接 HTTP SSE 探针，每组分别发送关闭与 medium：
Responses 两次 HTTP 200 且 response.completed，耗时 2946 / 4680 ms；
Messages 两次 HTTP 200 且 message_stop，耗时 715 / 816 ms。只输出状态、耗时和长度，
凭据保留内存；此探针不经过 Tauri/ModelClient，不能替代应用或 UI 端到端验收。

TypeScript SDK 基础 sdk.test.mjs 本次 15 passed；这不是 H011 的全部文件集合，
不能据此抹去其默认并行超时记录。workspace 全量测试在编译期间发现兼容问题后中止，
改跑最终源码的定向回归；完整 workspace、原生桌面构建及手动 UI 仍待完成。

AppCore 全量 lib 后续使用已编译测试二进制串行执行：1720 passed / 35 ignored / 1 failed，
耗时 829.46s（编译快照包含 H015，不含 H016）。五档输出上限测试
reasoning_tiers_keep_selected_model_and_registry_output_limit 通过；此前旧测试断言不发送输出上限，
已通过 RED 核实与注册表契约冲突，现断言所有五档均保持模型及注册表上限。
唯一失败仍为 project_runtime_wins_over_managed_fallback，实际 Local、预期 Managed；
未修改无关运行时选择逻辑，不能宣称 AppCore 全量或 workspace 门禁通过。

## H013：Messages 复用完整请求体预算（2026-11-18）

根因：Messages SDK 请求在最后直接 serde_json::to_string，绕过现有 48 MiB bounded writer。
先扩展既有 H69 矩阵，新增 Messages 路由并复用三协议完整 SSE 生命周期；正常控制断言实际文本。
RED：真实 48 MiB 转义输入未被拒绝，transport 返回成功，聚焦用例 0 passed / 1 failed。

修复：Messages 最后一步调用现有 request_body::encode，没有增加第二套预算。
GREEN：models/http/lib H69 同组四项 4 passed，包含全部公共入口、精确上限（含上限本身）、
工具、默认值、历史替换和真实 loopback HTTP。耗时 101.02 秒，未接触外部服务。
最终 models/http/all-targets clippy（offline，-D warnings）通过；测试 fixture 只在 lib 注册一次，
修复中途 clippy 指出的重复模块加载，未使用 lint allow 隐藏问题。

## H014：原生桌面构建依赖对齐（2026-11-18，验证中）

RED：独立桌面 workspace 解析 zip 2.6.1，calamine 0.26.1 的 ZipFile<'a> 与新增 R 泛型不兼容，
构建报 4 个错误。核对 calamine 当前 Cargo.toml（zip = 2）、两个版本真实源码和根 Cargo.lock，
根工作区已锁定兼容的 zip 2.4.2；桌面锁文件被忽略，单改本地锁无法保证可复现。

共享 zip 约束精确到 2.4.2，并将本地桌面锁对齐；cargo tree --locked --offline 确认
calamine、AppCore 与 rust_xlsxwriter 共用此版本。复跑已通过 calamine 编译，随后因缺少
项目要求的真实 CLI sidecar 停止。已启动既有 pnpm prebundle-cli 准备脚本；为先完成 H015 行为修复，
在依赖编译阶段中止，已有编译缓存保留，待最终源码稳定后重跑。
后续真实 pnpm prebundle-cli 通过，冷构建 20m57s；sidecar SHA256 为
2b16eb10ae095905802e65954c7c707e936c9daab1df3579e720668fa20fae03。
原生桌面 cargo build --locked --offline -j 1 后续通过，耗时 7m14s。
使用隔离数据根实际启动后，首次连接曾出现 Windows“未响应”；随后连接成功并进入向导。
用户亲自确认沙箱权限页后进入主界面，默认 DeepSeek 首轮真实对话成功，应用显示总耗时 14.0s。
初始化响应性仍待定位，双供应商产品流程继续验收中。

## H015：模型拉取保留配置与显式元数据（2026-11-18）

RED：真实服务与数据库用例 1 passed / 2 failed。再次拉取把已存模型的 display_name、protocol、
context_window、output_limit、capabilities 覆盖成默认值；新模型返回的明确容量和 effort 同样被丢弃。

修复：已有模型保持保存的配置，新增模型保留接口明确提供的容量、输入类型与 effort；
未声明能力仍按 H012 的默认五档处理。空/重复模型名、非法容量和非法能力在整批写入前拒绝，
不新增模型名称猜测。服务 18、resolver 12、registry run 7 项全部通过；H016 增加模型编辑后，
服务回归增至 19 项并通过。

## H016：模型配置编辑与跨层契约（2026-11-18，原生验收中）

RED：前端编辑器不存在，4 项用例失败；后端调用因输入类型与保存方法不存在而编译失败。
修复复用现有注册表存储：只编辑已存在模型的名称、协议覆盖、上下文、输出上限与思考能力，
保留激活状态及其他能力元数据。保存前校验正整数容量、输出范围和能力声明，坏输入不写库。
前端使用现有 shadcn Dialog/Input/Checkbox/Select/Button；保存失败保留表单和真实错误。
新增 Tauri 保存入口并同步 Rust/TS DTO；能力更新通知不触发余额查询。

GREEN：服务 19、resolver 12 项通过；前端编辑及实际 TS API 转发 5 项通过，tsc 通过。
编辑测试已加入 pnpm build。修改 Rust 文件 rustfmt 检查通过。完整前端构建与原生 UI 待收结果。

## H017：设置页供应商切换的异步边界（2026-11-18）

RED：真实组件回调的延迟 Promise 回归为 5 passed / 2 failed：切换供应商后旧行仍可编辑，
迟到的激活结果会修改另一供应商的同名模型。列表请求也缺少卸载/切换后的结果隔离。
修复：切换或新建时清空旧列表与编辑器；effect 清理后丢弃旧请求结果；激活回写使用
provider_id + model_name 联合身份。未新增缓存或第二套状态服务。

补充 RED：手动拉取的迟到回复仍可覆盖切换后的列表，7 passed / 1 failed。自动读取与手动拉取
统一使用同一个请求代次，切换和卸载均使旧代次失效。

GREEN：编辑/设置页 8、Composer 17 项合跑 25 passed，tsc 通过。完整 build 曾在 RED 阶段命中
前两项失败，修复后重跑通过，Vite 耗时 2m02s；最后新增的手动拉取代次保护在后续完整构建中复核。
原生桌面及实际 API 产品流程仍未验收完成。

## H018：思考档位的显式等价标注（2026-11-18）

RED：选择器未消费 thinking_distinct，明确声明的等价档仍只有普通标签，17 passed / 1 failed。
修复直接读取当前模型声明：仅在非空 distinct 声明中缺少的可用开启档标注“等价”，关闭档独立显示；
没有声明时不猜测等价关系。提交参数仍保持用户所选 canonical 值，不做隐式钳制。
同时移除本批引入的重复 thinkingMax 翻译键。

GREEN：Composer 18、模型编辑/设置页 8 项合跑 26 passed。最终 pnpm build 通过，
包含新增测试、tsc、Vite（1m29s）。真实新 sidecar 运行 SDK pnpm test:stdio，6 passed，
覆盖 cold fork、archive/resume、分页、方法路由、连接失败与 durable cursor；耗时 7397ms。
该组使用隔离 root 与既有离线运行夹具，不算外部模型或原生 UI 产品验收。原生截图仍待完成。

## H019：原生验收发现的模型设置标签缺失（2026-11-18）

RED：原生截图出现 settings.tabs.models，闭合的协议/余额选择器显示 chat_completions / auto。
根因分别为基础语言资源缺少导航键、菜单条目尚未挂载时 SelectValue 没有可用标签。
新增同一设置页组件与语言资源回归为 8 passed / 2 failed。
修复补充中英文导航名称，并用现有 SelectValue.placeholder 提供当前协议、余额与继承协议标签；
没有更改共享 Select 或协议值。GREEN：模型编辑/设置页 10 passed；原生复核与完整构建继续执行。

## H020：模型列表引用变化触发输入框渲染循环（2026-11-18）

原生控制台实际出现 Maximum update depth exceeded，调用栈定位 Composer。
根因：useComposerModelSettings 每次返回新 models 数组，Composer 的建议列表 effect 依赖该数组，
空输入时也 setSlashResults([])，形成渲染循环。新增公开 hook 的空闲重渲染及真实列表变化回归，
RED 为 18 passed / 1 failed。修复在 owning hook 按 state.options 缓存派生数组，
真实列表变化仍通知消费方；没有在 effect 内加额外状态特判。GREEN：Composer 19、设置页 10 项通过。
原生响应与控制台复核继续执行。

H020/H021 后续原生复验：模型菜单可打开并显示激活的 gpt-5.6-sol，选择后余额进入查询状态并返回 ∞；
五档均可选，选“关闭”后主按钮显示 gpt-5.6-sol 关闭。原生菜单不再触发本次选择器 SyntaxError。
最终完整 pnpm build（全部脚本、tsc、Vite）通过，Vite 耗时 3m47s。

## H021：原生模型菜单被复合模型标识触发崩溃（2026-11-18，验证中）

RED：实际原生应用打开模型菜单后白屏，DevTools 报 querySelector 的 SyntaxError，
堆栈为 SlidingPill → SlidingMenuList。注册表复合 id 使用 JSON 字符串，包含引号；
SlidingMenuList 将 activeId 直接插进 CSS 属性选择器，破坏了选择器语法。
在拥有选择器的共享菜单边界使用浏览器 CSS.escape 序列化值，保留模型标识及持久化契约。
此处以原生失败/复验作为回归，不用伪造 querySelector 的通过结果替代浏览器验证。

## H022：初始化与旧模型刷新移出原生命令分发线程（2026-11-18，验证中）

RED：本轮首次登录曾被 Windows 标为未响应。查证 Tauri 2.7.5 command wrapper：
同步命令在分发回调直接执行；initialize_project 中 block_on 网络发现后还同步写密钥/数据库并等待 Sandboxie 子进程，
旧 refresh_models 同样 block_on。此边界会在等待期间占用分发线程，不能以连接最终成功作为流畅性通过。
两入口改为 async 并复用已有 run_blocking；克隆现有 runtime handle，在 blocking pool 保持同一服务调用、
初始化顺序、错误与沙箱行为。未变更权限或另建执行链。Rust 编译、既有回归及原生响应性待复核。

## H023：真实 Responses 流被本系统序号入口阻断（2026-11-18，后续见 H025）

原生产品链真实请求失败，显示 invalid_response_sequence（previous=0, received=None），应用总耗时 16.2s。
同一授权目标的脱敏逐事件探针确认：response.created(0) → sub2api.gateway_ready(无序号) →
response.created(0) → in_progress(1) → 正常增量 → completed(9)。没有打印密钥、响应文本或请求正文。
现有 Responses accumulator 在所有 data 事件上先校验递增序号，把非 Responses 通知也送入了语义校验。
不能用此前 HTTP 200 探针宣称产品链通过。用户要求先查证 deepseek-harness 的第三方接入；
不再把本系统的入口缺口直接归咎于网关，也不新增供应商专用握手配置。查证和修复见 H025。

## H024：最终 workspace 门禁实测（2026-11-18）

cargo test --workspace --locked --offline -j 1 编译后在 AppCore lib 停止：1714 passed / 7 failed / 44 ignored。
失败为 4 项图像/语音无 EOF 超时、1 项 workflow oversized image 退出超时、1 项真实 compaction hook 启动超时、
1 项 project_runtime_wins_over_managed_fallback（Local != Managed）。
使用同一已编译测试二进制与断言串行复核相关组：25 passed / 6 failed；workflow 项通过，其余 6 项仍失败。
未更改超时或测试并发来伪造通过，也未修改独立画布及远程运行时逻辑。完整 workspace 门禁仍未通过。

## H025：第三方 Responses 入口与 Messages 基础地址（2026-11-18，原生复验中）

用户要求查证 deepseek-harness 第三方接入，并明确 Messages 表单只填写基础 URL，由系统追加资源路径。

依据：`借鉴/deepseek-harness/packages/llm/llm-pi-ai/src/adapter.ts:475` 将请求交给
`snapshot.models.streamSimple`；`models.ts` 将静态路由委托给协议实现。该参考版本锁定
`@earendil-works/pi-ai@1.3.0`，核对 npm 同版本发布包的 `openai-responses.js` 和
`openai-responses-shared.js`：以配置的 baseURL/apiKey 创建 OpenAI 客户端，按 type 消费事件，
没有本系统的强制递增序号门禁。不能声称我们已经完成 async-openai 迁移，P4 仍未完成。
Messages 推理路径依据同一参考项目 `llm-deepseek/src/messages-api.ts::messagesApiRoot`：
保留供应商前缀，缺省追加 `/v1`，已有末尾 `/v1` 则复用，再追加 `/messages`。
模型发现按提供方契约：通用 Anthropic-compatible 路由复用版本化根地址；
DeepSeek 官方 `/anthropic` 路由遵循 `llm-deepseek/src/listing.ts::modelListingURL`，使用同源 `/models`。
同一授权凭据实测 `/anthropic/v1/models` 为 404，`/models` 为 200，返回 2 个模型。
这一真实差异已补入失败测试；没有遇到 404 就盲试任意路径，也不向其他源转发凭据。

RED：原生逐事件失败序列的脱敏夹具再次复现 invalid_response_sequence；Messages 根地址测试
2 failed；AppCore 实际 discovery 请求为 `/anthropic/models`，与 `/anthropic/v1/models` 不符。

修复：Responses 首先分流标准 error 与非 response 命名空间通知；通知不进入语义序号和诊断载荷。
只有仍处于 created 元数据阶段时可重建创建基线，in_progress 或任何输出事件后继续严格验证，
不能用 created 重置已经开始的输出。此收敛是本地兼容边界设计，不新增 sub2api 分支或用户配置。
Messages 推理与通用模型发现共用一个 api_root；DeepSeek 官方的模型发现规则收敛在 discovery seam，
保存的用户 URL 保持原样，不要求用户填写 `/messages`。
同步三协议 HTTP 夹具和 runtime 请求断言，避免继续把旧的 `/messages` 根路径当作正确端点。

GREEN：models/http lib 280 passed；Messages 客户端 2、Messages stream 10、Responses sequence 9、
identity 9、terminal 5、旧序号审计 1、SSE framing 5，合计 321 passed。真实 reqwest 回环覆盖
扩展通知及重复 created，也保留输出后重复序号拒绝。AppCore 模型服务 20、内核终态 5 passed，
包含工具只提交一次及失败回放检查。Runtime H210 1、H55 三协议 4 passed；正式流完整性错误仍零重试。

H022 后端 `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --locked --offline -j 1`
实际 exit 0，17m57s。H025 原生二进制重建和真实 API/UI 复验进行中；全 workspace 门禁仍以 H024 的失败记录为准。

## H026：提交前定向回归复核（2026-11-18）

- `cargo test -p deepagent-models --features http -p deepagent-persistence --locked --offline -j 1 -- --test-threads=1`：644 passed、4 ignored、0 failed。
- `cargo test -p deepagent-app-core --test harness_model_resolver --test harness_model_service --test harness_registry_run --test harness_registry_seed --locked --offline -j 1 -- --test-threads=1`：51 passed、0 failed，包含三协议思考约束、启用目录、Slash 选择及当前 turn 冻结。
- 全部本轮已修改 Rust 文件的 `rustfmt --edition 2021 --check` 通过；全仓格式检查仍因未修改的 `crates/deepagent-core/src/id.rs` 四处宏排版失败。
- TypeScript SDK 的 `pnpm test`：204 passed；桌面 `pnpm build`：544 项测试、TypeScript 和 Vite 构建通过，另一次最终 TypeScript 检查与 Vite 构建通过。

本次是定向回归及提交整理，没有重新宣称完整 workspace 测试、全量 clippy 或所有真实供应商验收通过。原始本机验证日志位于忽略目录 `target/commit-audit-20261118/`。

## H031：请求准备复用边界（2026-11-19）

- 已提交运行环境探测持久化 `358a639e`，按用户确认跨软件重启复用，安装、卸载及路径变化后更新。
- 已提交 Git 上下文缓存与查询合并 `7f9af255`。按用户确认复用 30 秒，系统内文件或 Git 操作后立即失效；外部修改最多滞后 30 秒。目录链接、并发和查询期间失效均已回归，双协议六场景原生验收完成。详见 [Git 报告](reports/2026-11-19-git-context-cache.md)。
- 本轮进一步消除相同请求在压力检查、用量发布与发送前的重复计量；不改变配置、Skill、Hook 的刷新语义，也不跨 run 复用审批、权限或取消对象。三个行为回归先 RED 后 GREEN，另补三协议、并发及图片边界。详见 [计量复用报告](reports/2026-11-19-request-measurement-reuse.md)。
- Context 71 项通过；Runtime 全量 501 通过、1 项未修改的 ACL 恢复测试失败，单独复核仍失败；最终排除该项后的 501 项通过。相关范围严格 Clippy、定向 rustfmt 及最终原生构建通过，全仓离线/格式/依赖 lint 的阻塞按报告保留。
- 最终真实双协议六场景全部完成，运行环境缓存记录未重写。续聊准备为 Responses 760 ms、Messages 918 ms；发送前计量为 2～4 ms。未达到整体 10 ms，不将两轮整体耗时差异全部归因于计量优化。
- 完整方案、思考参数模型矩阵及其余 Harness 验收继续进行，以上性能工作不代表完整目标已完成。

## H032：运行内工具审计投影复用（2026-11-19）

- `run_capabilities.rs` 复用准入已计算的模型 schema，目录以完整定义精确核验后复用单工具哈希；模型侧与协议侧各保留最近一组输入和投影。协议计算和发布共用运行内锁，相同并发输入只计算一次。
- 所有命中仍经过 `RunStore::record_capability_schema`，终态、缺失持久化、独立写入者冲突仍可阻止发送；持久化成功后才更新本地 generation。工具发现、定义、顺序、模型、协议和实际 wire 参数变化均重新投影，不冻结下一次工具发现。
- 将待提交的工具类型与名称解析收敛到现有 provider codec，修复 Messages 普通工具和排序后的来源关联；这部分与思考参数或供应商缓存策略分开提交。
- 初始三项行为 RED，复用接入后三项 GREEN；原有 12 项审计回归通过。补充定义篡改、顺序、路由、数据库读取失败及独立写入者覆盖，重构后继续回归。
- 本次完整 AppCore 库测试为 1742 通过、1 失败、36 忽略。失败用例假定 Node 24 会使用托管运行时，但当前 PATH 上已有 Node 24.18.1，生产逻辑按本地优先选择它；仅在独立测试进程移除宿主 Node 路径后该项通过。未修改生产运行环境逻辑，不能宣称原完整命令全绿。
- 首次重叠测试重编译被 Windows 运行中 exe 占用，LNK1104；后续改为串行执行，不计作行为失败或成功。
- 50 工具重复 100 次的局部基准中，模型侧与 wire 侧耗时约减少 91%/95%；不能外推为整体请求改善。独立六文件源码快照构建通过，相关回归 18 通过、1 忽略；该快照 Responses 四场景通过，Messages 四场景受尚未提交的协议修正缺失阻塞，失败完整保留。
- 当前集成工作树原生构建通过，真实双协议八场景全部完成，两个工具续写均核对到指定文件读取及再次模型调用；wire 审计分别为 76→22 ms、56→17 ms，运行环境缓存未重写。续聊准备仍为 1647/1236 ms，未达到 10 ms，也不能声称端到端比前轮更快。
- 全仓离线检查曾因并行修改的文件编译失败，严格 Clippy 和整仓格式仍有其他问题；未用原生构建成功替代这些结果。详细版本边界、耗时和后续慢点见 [投影复用报告](reports/2026-11-19-capability-projection-reuse.md)。本轮不代表完整方案完成。

## H033：目录扫描去重与工具准备分段（2026-11-19）

- `WorkspaceScanner::walk` 在排序和输出时复用每个条目的一次目录分类，保留排序、忽略、链接跟随和遍历上限，不引入跨扫描缓存。回归先 RED（50 条目查询 214 次）后 GREEN，完整 workspace crate 25 通过、1 个显式基准忽略；严格 Clippy 通过。
- 20 次真实仓库扫描的目录查询从 28200 降为 5380，累计时间从 388222 µs 降为 117608 µs，输出字节数相同。这是局部基准，不代表整轮请求达到 10 ms。
- 沿用现有内存累计模式记录工具准备五段，模型绑定单独附在既有 `registry_ready` 事件；不记录正文、schema、路径或凭据。计时回归先 RED 后 GREEN，普通聊天与提前准入/消费路径通过，每次实际准备只记录一次。
- 当前集成工作树原生构建成功，真实 Tauri 双协议四场景均完成且回复预期，运行环境两份缓存未改写。Responses / Messages 续聊准备分别为 929 / 1050 ms；工具构建为 65～100 ms、能力元数据为 33～105 ms、能力快照为 73～120 ms、模型绑定为 1～29 ms。首次 Git 查询仍为约 1.61 秒。
- 完整 AppCore 库为 1743 通过、2 失败、36 忽略：已知宿主 Node 优先级断言仍失败；验证生命周期夹具等待共享构建目录超时，取消继承目标目录后独立复跑通过。整仓格式仍有未修改的 `id.rs` 排版差异，不宣称门禁全绿。
- Git 规则继续保持 30 秒复用、系统内操作立即失效、外部修改最多滞后 30 秒。当前没有每轮重新探测运行环境的证据；尚未完成同负载端到端 A/B，不能把整体差异全部归因于扫描优化。详见 [扫描与计时报告](reports/2026-11-19-workspace-entry-scan.md)。完整目标继续进行。

## H034：完整协议工厂与 Responses SDK 类型层（2026-11-19）

- 对照 DeepSeek Harness 的 `PROTOCOLS`、`buildProvider` 和 `createProvider`，由单一协议表选择完整 codec。Responses、Chat Completions、Messages 的请求准备、编码、发送入口及 decoder 均归属 codec，普通发送、取消和 Files 有限回退共用一条物理传输路径；每次物理发送创建独立解析器。
- 生产 Responses 序列化接入精确固定的 `async-openai 0.43.0`，仅启用 `response-types`。标准请求、消息、工具调用与结果、图片、工具、reasoning、text、tool_choice 使用 SDK 类型；DeepSeek 的 `user`、非 SDK effort、reasoning 历史及原生工具扩展集中显式适配。
- 沿用 P0 已确定的类型与序列化边界，HTTP/SSE、UTF-8、语义聚合、取消、usage、终态及 48 MiB 完整体预算继续复用。未引入 SDK 网络客户端，没有新增过载重试。
- 工厂架构边界先 RED 后 GREEN；迁移中复现并修复 `f32 -> Value -> f64` 导致的采样参数精度变化。集成工作树 models/http 全量 432 通过、0 失败、4 忽略；独立提交快照新增专项 7 通过，不依赖其他待提交的思考、缓存或 UI 改动。
- 其余验证、独立提交版本边界和未覆盖范围见 [本轮报告](reports/2026-11-19-protocol-factory-responses-sdk.md)。本轮文件定向格式检查通过，全仓格式仍有未修改 `id.rs` 的既有差异；不以模型专项通过宣称完整方案或全仓门禁完成。

- 独立快照既有回归再通过 307 项，与新增专项合计 314 项；独立快照严格 Clippy 通过。完整 workspace 实际结束为 5757 通过、61 失败、130 忽略，13 个目标失败；仓库外 TEMP 复核解释其中 9 项，但不改写完整命令结果。集成工作树严格 Clippy 被本轮基线已有的 `thinking.rs` 单元素循环阻断。其余固定路径、平台权限、配置/思考/容量及进程测试失败完整留在报告，未全部归因，不宣称全仓门禁通过。

## H035：Messages 图片与原生鉴权（2026-11-19）

- P2 工厂保持单一实现，增加 codec 所属鉴权类型，并同时用于推理与模型发现。
- 使用已固定 Anthropic SDK 的 image/source 类型补齐四种图片格式、远程 URL 及工具结果图片；复用异步字节校验、取消、模型能力门禁和 48 MiB 完整请求预算。
- Messages 同时携带 API Key、Bearer 和 2023-06-01 版本头；真实回环 HTTP 验证推理与发现行为。原生鉴权客户端只允许同源重定向，错误中的密钥回显与请求 Debug 均脱敏。
- 初始图片与请求头用例先失败；实现后新增图片/HTTP 与工厂专项 11 通过，模型集成二进制 33 个目标合计 155 通过、4 忽略，独立提交快照严格 Clippy 通过。本轮文件格式通过；全仓格式仍有 `id.rs` 既有差异。workspace 在 `kernel_v2_e2e` 编译时内存不足退出，未完成测试。最终独立提交快照完整 models/http 为 420 通过、0 失败、4 忽略；纯图片空文本块回归先失败后修正，UTF-8 夹具已迁移到受保护的客户端构建器。最终集成专项 12 通过，AppCore 模型服务回归 25 通过。
- 本轮隔离提交，不纳入工作树已有的思考、缓存、视觉治理或 UI 改动。自定义 Rust 传输的字段/方法兼容说明、完整验证与未覆盖范围见 [本轮报告](reports/2026-11-19-messages-images-native-headers.md)。

## H036：P11 真实矩阵、F0 复核与 F1+ 补齐（2026-11-19）

- 本轮生产 ModelClient/协议工厂/ReqwestTransport 对 Layso Responses、Messages、Chat Completions 完成相同 11 场景，最终 33/33 通过。覆盖文本、历史、工具续写、五档思考参数、图片、取消与缓存观察；三协议均实际读取缓存。此结论限定适配层矩阵，未替代完整桌面验收。
- 早期失败保留：缺失测试图片能力声明、不合适的 Chat 思考测试配置、一次连接失败；指定工具名的 Responses HTTP 400 经独立 HTTP 请求复现，auto 成功。未新增生产重试或供应商特判。
- 另加跨协议历史矩阵：首轮 1/3，最终两轮 2/3。Claude 带签名历史→Chat、Responses 历史→Messages 已通过；Claude→Responses 三轮均在来源 /messages 请求阶段收到 api_error，未发到目标 /responses，仍未验收。该矩阵含 item 序列化往返，不能外推完整 SessionStore/origin 或不同供应商迁移。
- F0 已复核 16 个方法、消费方及连接/steer/终态/审批/游标边界。F1+ 为全部出站机器事件补 protocolVersion；Rust/TS 校验明确错误版本、保留旧 v1 兼容；补全 TS 事件类型和 sandbox/status 专项。
- 持续回放为明确的本地 SDK 设计：复用 thread/read，追平后每 250ms 查询；checkpoint 绑定线程并分开保存每条持久流的序号。没有新增 turn/stream、存储、自动重连、ACK 或变更重试；真实 CLI 验证重连与去重。
- TypeScript SDK 212 项、本轮真实 stdio 6 项和真实 JSONL 1 项通过。模型/http 本地 441 通过、0 失败、5 忽略。app-server 138 通过、3 项思考旧断言失败、5 忽略；保留失败，未擅自重写模型规则。
- Rust SDK 最终单元 88 通过/3 忽略、真实 stdio 15 通过；CLI JSONL 3 通过。workspace、严格 Clippy 和全仓格式门禁未通过；细分结果及原始现象写入报告，没有用专项通过代替完整门禁。
- 完整命令、最终门禁、测试配置及排除范围见 [本轮报告](reports/2026-11-19-p11-f0-f1-acceptance.md)。不将本轮结果解释为完整方案或全仓门禁完成。
