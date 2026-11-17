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
