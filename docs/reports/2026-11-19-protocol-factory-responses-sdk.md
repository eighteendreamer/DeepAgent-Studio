# P2 完整协议工厂与 P4 Responses SDK 类型层

日期：2026-11-19。范围仅为 P2/P4，不代表多协议方案整体完成。

## 依据与边界

- 本地 `借鉴/deepseek-harness/packages/llm/llm-pi-ai/src/provider.ts:48-53,178-191`：协议表返回完整的 ProviderStreams 实现。
- 同目录 `models.ts:50-51`：stream/streamSimple 直接委托工厂产生的 api。
- Rust 的 WireCodec/StreamDecoder 是上述边界在本项目的本地设计，不复制参考项目的业务代码。验证覆盖协议选择、预算/实际发送一致性、解码结果和已有取消/Files 行为。
- 遵循方案 P0 已决定的边界：SDK 只承担类型与序列化；现有 HttpTransport、SSE 分帧、UTF-8、取消、语义聚合和最终请求体预算继续使用。
- async-openai 精确固定 0.43.0、MIT，仅启用 response-types。以实际下载的版本源码为准，不将本地借鉴副本当作依赖。

## P2

`protocol::PROTOCOLS` 统一注册 Responses、Chat Completions、Messages。每个 codec 负责请求准备/编码、发送入口、decoder、采样/图片策略。ModelClient 的普通和取消入口均委托同一 codec；显式 Chat API 使用同一个 Chat codec。

`ModelClient::stream_attempt` 是所有协议和有限 Files 回退的共同物理发送路径。每次物理发送创建独立 decoder，保留终态、usage、工具身份、流序号和取消语义。Files 无效引用仍只允许原有有界回退；没有新增过载重试。

架构边界回归在修改前真实失败、修改后通过。移除了此前仅扫描 imports、未覆盖生产实现的无效协议分支测试，改为扫描实际 ModelClient 和 Files 实现。

## P4

Responses 生产序列化使用 SDK 的 CreateResponse、EasyInputMessage、FunctionToolCall、FunctionCallOutputItemParam、CustomToolCallOutput、InputContent、Tool、Reasoning、ResponseTextParam 和 ToolChoiceParam。

以下差异在同一 adapter 中显式保留：DeepSeek user、自定义 wire effort（例如 ultra）、没有 summary 的 reasoning 历史、没有 SDK 必填 id 的 custom call、没有 format 的 custom tool、文件引用省略 detail、原生工具发现和搜索扩展。不会把完整输入反序列化为 SDK 的 InputItem：其 ItemReference 未标记分支可能仅保留 id，吞掉历史内容。

迁移回归发现 Value 将 SDK f32 先转换为 f64，导致实际发送的 0.95 变成 0.949999988079071。保留两个 SDK 浮点字段的直接最终序列化后，原先失败的三项断言恢复通过，并增加精度回归。

最终序列化仍受原有 48 MiB 完整体预算约束，定价/审计工具投影和实际发送共用此实现。没有新增 SDK 网络客户端，没有更换供应商地址、密钥或模型配置。

## 验证记录

- 集成工作树：`cargo test -p deepagent-models --features http --locked --offline --no-fail-fast -j 1`，432 通过、0 失败、4 忽略（含 doctest）。覆盖三协议回环 HTTP、UTF-8、SSE 生命周期、取消、Files 有限回退、图片、工具、usage、流序号、终态和完整请求体预算。
- 独立提交快照：`protocol_factory` 与 `responses_sdk` 共 7 通过、0 失败，证明工厂和 SDK 迁移不依赖其他待提交代码。
- 独立快照既有回归：库测试、`reasoning_effort_audit`、`modalities_audit`、`messages_client`、`sse_lifecycle` 共 307 通过、0 失败；包括真实回环 HTTP 与活动流取消。与新增专项合计 314 通过。
- 独立提交快照：`cargo clippy -p deepagent-models --features http --all-targets --locked --offline -j 1 -- -D warnings` 通过。
- 集成工作树严格 Clippy 未通过：已有未提交 `thinking.rs:666` 的单元素循环触发 `clippy::single_element_loop`，该代码在本轮开始的基线中已经存在。本轮独立提交快照严格检查通过，未顺手修改其他任务的思考测试。
- 完整 workspace：`cargo test --workspace --locked --offline --no-fail-fast -j 1` 实际执行结束，按顶层 suite 汇总为 5757 通过、61 失败、130 忽略，13 个测试目标失败。汇总排除了失败输出中重复打印的子测试结果；完整命令未通过。
- AppCore 库为 1746 通过、4 失败、46 忽略：其中 Git 非仓库识别和两个 Cargo 目录检测用例，是本轮将 TEMP 放在仓库内导致；更换仓库外 TEMP 后三项独立复跑通过。Node 优先级断言在仓库外 TEMP 仍失败（实际 Local、期望 Managed），与现有宿主 Node 环境相关。
- 同一 TEMP 问题还影响 Workspace 的 4 项、Builtins 的非仓库用例及 Subagents 的无效仓库用例。仓库外 TEMP 复核为 Workspace 全量 25 通过、1 忽略，后两项各自通过。共 9 项原失败已复核恢复，保留完整命令中的原始失败计数，不将复跑覆盖为全仓通过。
- `golden_trace` 三项在创建固定 `G:\Code\Kotlin_code\_deepagent-e2e` 目录时失败；部分旧 `kernel_v2_e2e` 用例共用该固定目录。失败发生在模型发送前，未为此次验证修改业务代码或建立机器专属盘符映射。
- 本轮 Rust 文件定向 rustfmt 检查通过。`cargo fmt --all -- --check` 未通过，差异仅在未修改的 `crates/deepagent-core/src/id.rs` 四处宏参数排版。
- 初次默认并行构建触发 Windows 页面文件不足（os error 1455）；改为单编译任务后模型层全量通过，不将该次环境失败计作行为回归。
- SDK feature 检查确认仅启用 `response-types`，未启用 `_api` 网络层。以上本地日志位于忽略目录 `target/p2-p4-audit/`，不随提交发布。
- 本轮没有前端或 Tauri 契约变更，未执行桌面重编译和 UI 验收；没有调用真实远程模型 API。

### 全仓失败目标

这是含其他待提交改动的集成工作树结果，不是独立提交快照结果。除已经复核的 TEMP 问题外，未建立完整基线对照，不声称其余失败全部由环境或旧代码造成，也不将本轮扩大为这些模块的修复。

| 目标 | 通过 / 失败 / 忽略 | 已观察到的边界 |
|---|---|---|
| AppCore lib | 1746 / 4 / 46 | 3 项 TEMP 复核通过；Node Local/Managed 断言仍失败 |
| AppCore golden_trace | 0 / 3 / 0 | 固定 G 盘目录不存在，未执行到模型发送 |
| AppCore kernel_v2_e2e | 345 / 28 / 7 | 固定目录、模型注册/思考/容量预期、DACL 恢复、MCP 等待超时等，未全部归因 |
| AppCore mcp_environment | 0 / 1 / 1 | 隔离环境测试宿主失败，未独立复核 |
| Builtins lib | 375 / 4 / 3 | 3 项符号链接权限错误 1314；1 项 TEMP 复核通过 |
| CLI bin | 223 / 3 / 7 | 配置、无效思考档位和 high/medium 预期不一致 |
| CLI session_startup_isolation | 2 / 1 / 1 | 选择/恢复断言未通过，未独立复核 |
| Harness SDK e2e_stdio | 14 / 1 / 0 | 自省预期供应商 deepseek-official，实际 null |
| MCP env_inheritance_audit | 0 / 5 / 1 | 隔离环境测试宿主失败，未独立复核 |
| MCP process_lifecycle_audit | 0 / 5 / 0 | 进程生命周期用例失败，未独立复核 |
| Runtime lib | 502 / 1 / 3 | 检查点测试恢复后的 DACL 不一致 |
| Subagents lib | 24 / 1 / 0 | 无效仓库用例在仓库外 TEMP 复核通过 |
| Workspace lib | 21 / 4 / 1 | 仓库外 TEMP 全量 25 通过、1 忽略 |

## 交付隔离

工作区已有未提交的思考策略、Messages 缓存、视觉路径、Skill 和 UI 变更。本轮基线已单独保存；提交快照从 HEAD 构造，只包含 P2/P4 及相应文档，另行构建核验。集成工作区保留已有功能，不用它的成功掩盖独立提交的缺失依赖。

## 未覆盖范围

未在本轮调用真实远程模型 API；回环 HTTP 和合成流验证本系统发送/解析边界。Messages 图片、原生 Anthropic 请求头、真实缓存命中率、全模型思考参数矩阵及 F1+ 不属于本轮完成项。
