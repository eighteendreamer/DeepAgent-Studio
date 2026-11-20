# H037：三协议工厂、具名工具与桌面验收（2026-11-20）

## 实现边界

Responses、Chat Completions、Messages 均注册在 `deepagent-models/src/protocol.rs` 的同一张协议表，由 `codec_for` 分发。codec 负责协议请求准备、编码与语义流解析；三条路线共用 `ModelClient::stream_attempt`、原有 HTTP/SSE、取消与调用计量，没有另建 agent loop、存储或重试机制。

本轮为 Chat 的标准请求接入固定版本 `async-openai 0.43.0` 的 `CreateChatCompletionRequest` 和消息 DTO。DeepSeek 的 `thinking`、`reasoning_content`、`user_id`、文件引用等扩展经过显式保存与恢复。SDK 负责类型与序列化，项目原有状态机继续处理流式增量、用量和终态；这不代表用 SDK HTTP 客户端替换原传输。

通用 Chat 使用 `max_completion_tokens` 和 `user`，不夹带 DeepSeek 专属字段。最终提交复核还发现：旧配置的明确关闭被当作 DeepSeek 的省略规则处理。新增真实编码入口回归先失败，再将该省略规则限制在官方 DeepSeek 路由；通用 Chat 的两种明确关闭入口都保留 `reasoning_effort: "none"`。此项没有引入新的思考档位或模型能力声明机制。

采样参数也按 Responses 的既有处理保留到最终序列化。新增回归先证实 Chat 的 `0.2` 被输出为 `0.20000000298023224`，再修正 SDK/JSON Value 转换引入的精度展开；`0.2` 与 `0.95` 恢复原有 wire 表示。失败和修正后 3 项通过分别见 `chat-sampling-red.log`、`chat-sampling-green.log`。

参考依据：

- `借鉴/deepseek-harness/packages/llm/llm-pi-ai/src/provider.ts`：协议表选择完整实现。
- `借鉴/async-openai/async-openai/src/types/chat/chat_.rs`：Chat 请求 DTO。
- `借鉴/Tianshu-harness/src/api/responses-client.ts`：指定函数关闭并行调用。

## 具名工具 HTTP 400 的定位

原请求在 Responses 与 Chat 均返回 HTTP 400，`auto` 控制组成功。保持工具定义、模型、函数选择与提示不变，只增加 `parallel_tool_calls: false` 后，两协议都成功，且只返回指定的 `audit_named_tool`。

生产修复在两个协议各自的序列化边界完成：具名函数关闭并行；`auto`、`none`、`required` 保持原状。没有把具名选择改成 `required`，没有捕获 400 自动重发，也没有删掉其他工具定义。

`named_parallel_tools` 覆盖两协议 × 请求/默认配置 × 普通/带取消入口 × 四种 choice，共 32 组。断言请求只有一次、工具列表完整、choice 正确以及并行开关范围。

忽略目录证据：`target/p11-f1-audit/named-parallel-current.log` 为单变量对照；`named-parallel-green.log` 为回归结果；最终产品真实请求另见 `named-product-final.log`。

## 历史转换

旧报告中 Claude→Responses 的失败发生在来源 `/messages`，当时尚未向目标 `/responses` 发请求。补验使用当前可用的 `claude-fable-5-2` → `gpt-6.2-sol`，连续三轮来源与目标均完成。最终三轮的来源签名数分别为 0、0、1；不能要求供应商每次都返回思考签名。

这一结果只证明该路线的 item 序列化与恢复，不能代替所有模型、供应商组合以及完整 SessionStore/origin 迁移验收。证据在 `target/p11-f1-audit/history-product-final.log`。H036 的原始失败记录保留，不改写为当时成功。

## 缓存

参考 DeepSeek Harness 的稳定前缀思想和天枢的 Messages 断点：

- Messages 工具按名称稳定排序，在工具末尾设置断点。
- 稳定 instructions 与动态 system 环境项分开，只标记稳定指令。
- 已完成历史的合法文本或工具调用块可设置断点；不标记思考签名块与当前输入。
- DeepSeek 官方 Messages 地址使用自动缓存，不额外发送显式 `cache_control`。
- 继续保留三协议缓存读取、创建、未命中及 reasoning usage。

本轮重复请求体的供应商观察值：Responses 0/11,250，Chat 11,008/11,250（97.85%），Messages 23,952/23,960（99.97%）。这些是特定模型和路线的返回值，前缀曾被使用，不能当冷缓存基准或承诺所有请求的命中率。

DeepSeek Harness 的 `cacheRetention` 路由配置、签名思考与前缀摘要的持久绑定没有在本轮完整复现。这些能力与供应商缓存命中不同，不能用命中 token 证明已实现。

## 桌面 UI

验收通过脚本连接独立原生 Tauri WebView 的 CDP 9224，使用隔离数据根、项目、会话及 WebView 目录。脚本先选其他模型，再点击实际胶囊、从列表选择目标模型并关闭菜单，核对持久化选择后输入文本、按 Enter 并监听真实运行终态。请求通过本地记录代理转发到用户指定的 Layso。配置和选择在 finally 恢复；密钥、提示与模型正文不写入验收产物。

旧构建的三协议 UI 对话均 HTTP 200 并完成：Responses 约 6,946 ms、Chat 约 8,707 ms、Messages 约 4,695 ms，零 JS 异常、零恢复失败。它们不包含本轮具名工具修复，不能替代新构建验证。

Messages 首次 UI 尝试在输出上限 131,072 时收到 400；将隔离测试模型上限调整为 4,096 后成功。第二次同时更换了隔离 workspace 与合成提示，因此不能断言输出上限是唯一原因。测试后恢复模型设置，不擅自改全局输出策略。

包含关闭思考与采样精度修正的最终 custom-protocol 原生构建完成后，重新执行脚本，并检查三张截图中都显示了实际回复 `P2_UI_OK`：

| 协议 | HTTP | 选模观测 | UI 完成耗时 | Start→Chat DOM | 页面 loading | 运行终态 |
|---|---|---|---|---|---|---|
| Responses | 200 | 485.2 ms | 14,492.5 ms | 155.9 ms | 无 | completed |
| Chat Completions | 200 | 330.5 ms | 16,750.8 ms | 101.5 ms | 无 | completed |
| Messages | 200 | 396.4 ms | 9,601.4 ms | 107.7 ms | 无 | completed |

零 JavaScript 异常、零配置恢复失败。选模观测包含菜单操作、持久化与 100 ms 轮询，不是纯事件处理耗时；UI 完成耗时包含供应商等待及轮询观测，不能视为页面切换耗时。当前测试负载下未达到此前提出的 10 ms 页面响应目标。这三次 UI 截图的缓存计数均为 0，上面的重复请求体命中数据来自不同样本，不能外推为 UI 对话命中率。

记录代理还观察到余额、标题与后台辅助调用。`requestCount` 按模型名计数，会包含上一轮辅助调用；它不是某次 turn 的物理重试计数，不能据此认定协议自动重发。三条主对话请求都带 49 个工具，按相应协议路径完成。

本地证据在 `target/p11-f1-audit/ui-precision-final.log`、`ui-precision-summary.json`、`ui-protocol-smoke.json` 和 `ui-{responses,chat_completions,messages}.png`，不提交数据根、截图、密钥或请求正文。验收结束只关闭测试进程，原用户窗口保留。

## 门禁记录

- `pnpm build`：通过脚本测试、TypeScript 与 Vite 构建；仍有既有大 chunk 提示。
- `cargo fmt --all -- --check`：集成工作树与最终独立源码均通过。
- `cargo clippy --workspace --all-targets --all-features --offline -j 1 --keep-going -- -D warnings`：采样精度修正后最终退出 0，见 `clippy-precision-final.log`；Cargo manifest 仍提示既有未使用依赖，Rust/Clippy 检查通过。修正测试锁作用域、单元素切片克隆、不必要借用与整数整除写法，未放宽 lint 门禁。
- workspace 首次尝试在编译阶段发生 Windows 页面文件不足；单任务重试又出现 `rustc-LLVM ERROR: out of memory`。两次都不能计为测试用例失败或通过。
- 后续 workspace 使用 `-j 1` 及 AppCore 单包 `debug=0` 降低编译内存，测试断言和产品运行行为不变。
- 最终桌面命令为 `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --features custom-protocol --offline -j 1 --config 'profile.dev.package.deepagent-app-core.debug=0' --config 'profile.dev.package.deepagent-desktop.debug=0'`，退出 0，见 `desktop-precision-final.log`。首次桌面构建也受分配失败影响，降低调试符号并错开构建后成功。
- 最终独立提交源码的 `cargo test -p deepagent-models --features http --offline -j 1 --no-fail-fast`：37 个测试目标合计 431 通过、0 失败、6 忽略，见 `selected-models-precision-final.log`。同一源码 HTTP/all-targets 严格 Clippy 退出 0，见 `selected-clippy-precision-final.log`。源码快照排除了工作树内其他未完成的思考声明、视觉和 UI 改动。
- 两个 DeepSeek HTTP 夹具原本使用 `127.0.0.1` 却断言专属 `thinking` 字段；通用 OpenAI 字段隔离后该断言失败。现保留 `api.deepseek.com` 主机名，并通过 Reqwest 的测试 DNS 解析到回环服务器，继续验证官方扩展；通用路由另有明确的字段排除回归。注册表运行测试同步检查 Chat 的 `max_completion_tokens`，没有改变产品的路由或参数校验来迁就旧断言。
- 集成工作树 `harness_registry_run` 修正输出字段断言后，10 通过、0 失败，见 `registry-verified.log`。旧配置关闭的失败证据在 `chat-off-red.log`，最终回归包含在上述 431 项中。
- 集成工作树的实际 Chat 编码边界再验 5 通过、0 失败，见 `current-chat-boundary-verified.log`。
- 已修复两个 E2E 夹具写死不存在的 G 盘路径，改用系统临时目录；修复当前 Rust/Clippy 指出的等长分块、原子 ID 分配、无效 format 及简单借用问题。原子 ID 仍保留溢出错误与并发唯一性，不采用超出 Rust 1.88 的新 API。
- 本次完整 workspace 命令退出 101：206 个目标合计 5799 通过、36 失败、132 忽略，共 9 个失败目标。历史“11 个目标、60 个用例失败”是 H036 的结果。

## 完整 workspace 的未通过项

以下保留完整命令的原始结果。注册表的两项随后已修正并定向复验通过，但没有再跑完整 workspace，不能直接把原始总数改成 34 项。

| 目标 | 通过 / 失败 / 忽略 | 当前核实的现象 |
|---|---:|---|
| app-core lib | 1749 / 1 / 46 | 宿主 PATH 已有兼容的 Node 24.18.1，运行时按本地优先选择 Local，夹具却期待 Managed；需要隔离宿主 PATH |
| app-core harness_registry_run | 8 / 2 / 0 | 旧断言遗漏 Chat 的 `max_completion_tokens`；修正后单独 10 / 0 / 0 |
| app-core kernel_v2_e2e | 358 / 15 / 7 | 未注册模型、预算错误文本、旧思考与容量预期、输出上限恢复预期不符，分组见下文 |
| app-core mcp_environment | 0 / 1 / 1 | 隔离环境宿主失败，内部原因本轮未进一步验明 |
| builtins lib | 376 / 3 / 3 | 三项文件符号链接夹具返回 Windows 错误 1314：客户端没有所需特权 |
| CLI bin | 224 / 3 / 7 | config/read 与 thread/turn 思考配置的旧断言不符 |
| CLI session_startup_isolation | 2 / 1 / 1 | `h89-unknown-thread` 未注册，初始化返回 unknown model |
| MCP env_inheritance_audit | 0 / 5 / 1 | 后续用相同合成环境单独运行 child，确认 Node 被本机 NVM 以 NVM4301 拒绝启动：改写 HOME/USERPROFILE 后安装根配置无效；未更改 NVM 或放宽环境隔离 |
| MCP process_lifecycle_audit | 0 / 5 / 0 | PowerShell 执行策略拒绝临时 `.ps1` 夹具，以及子进程启动等待超时 |

内核 15 项中：3 项成本场景明确返回 `deepseek-v4-pro` 未注册，1 项图片场景返回 `h71-text` 未注册；另 2 项成本场景只确认预期的预算错误文本不符。其余为 5 项思考配置预期及 4 项容量、输出上限和恢复次数预期不符。没有在清洁 HEAD 隔离所有剩余失败，因此不把它们全部称为既有问题，也不通过放宽生产模型校验或修改系统安全设置消除失败。

该完整命令在最终旧配置关闭映射与采样精度修正之前开始；最终独立源码的 431 项及集成边界 5 项是后续验证。运行时库本轮为 503 通过、0 失败、3 忽略；不能沿用早期运行时失败记录。原始证据在 `workspace-final.log`、`workspace-final-summary.json`，后续注册表修复与完整命令的版本边界分别列明。

收尾定向验证：历史回滚 9、会话笔记 1、模型服务/注册表/初始化 43、输入历史处理 37、系统提示前缀 12、历史文件分页 15 项通过。搜索测试必须启用 `web`，默认功能的 0 项不计为验收；启用后为 5 通过、1 失败，该失败因 `deepseek-v4-pro` 未注册，本次改动涉及的设置持久化及真实搜索续写均通过。见 `close-*.log` 与 `search-web-final.log`。NVM 合成环境诊断另见 `environment-child-diagnosis.log`；AppCore 隔离 child 单独复验仍在 connected 状态断言失败，没有取得它的内部错误，不能直接把该项也归因于 NVM。
