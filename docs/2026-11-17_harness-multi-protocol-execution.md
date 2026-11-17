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
