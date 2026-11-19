# 发送前上下文耗时分项（2026-11-19）

## 结论与范围

用户要求排查发送前 2～7 秒是否来自重复初始化，并决定将 Node/Python/Java 的版本探测结果落盘、在安装、卸载或路径变化后更新。运行环境复用已在 `358a639e` 实现，详见 [运行环境缓存报告](2026-11-19-runtime-probe-cache.md)。本次继续细分上下文成本，不修改 Git 快照的新鲜度、供应商、协议、模型、思考档位、权限或沙箱策略。

真实桌面采样表明：预热后的上下文成本主要来自每条用户消息同步执行的 Git 查询；不是运行环境版本重新探测，也没有证据把该部分时间归因于模型推理。初次登录与基础设置不计入测量。本次仍有明显本地准备成本，未达到 10ms，也不代表整体性能目标或双协议方案全部完成。

## 计时边界

沿用 `run_requested → 首次 model_agent_transport_invoked` 的总准备口径。在既有 `build_run_context` 中记录八个互不重叠的单调时钟区间：配置及目录声明、系统环境、Git、项目结构、指令及 manifest、渲染、用户附件、初始 token 用量估算。

本地设计：各区间只在内存累计微秒数，完成后通过既有日志入口合并写入一条 `run_context_profile`；区间内不逐项写诊断数据库。数据只有固定阶段名和耗时，携带既有 run/session 关联，不包含路径、提示词、指令正文、凭据或模型正文。既有 `run_context_built` 仍表示上下文合计，不能再与八个子区间相加。

## 原生桌面实测

重新编译 debug 桌面成功，耗时 3分31秒。编译和回归测试结束后，通过 Playwright/WebView2 CDP 操作真实 Tauri 窗口，未使用桌面控制插件。工作区为当前 DeepAgent-Studio 仓库，采用既有已登录隔离数据根。

| 样本 | 总准备 | 上下文合计 | Git | 项目结构 | 终态 |
| --- | ---: | ---: | ---: | ---: | --- |
| 本次重启后 Responses 新会话 | 6002ms | 2780ms | 2267.607ms | 430.458ms | completed |
| Responses 同会话续聊 | 2863ms | 1541ms | 1353.127ms | 162.164ms | completed |
| Messages 新会话 | 3055ms | 1660ms | 1484.997ms | 157.582ms | completed |

首次样本单独保留，不能用预热后的两个样本掩盖它。软件初始化登录早于 `run_requested`，因此未计入上述区间；首次发送仍有较高准备成本，不据此宣称已定位全部冷启动差异。样本不是受控硬件基准，不把本次数据与此前样本的差异当作优化净收益。

预热后其余上下文分项：

| 分项 | Responses 续聊 | Messages |
| --- | ---: | ---: |
| 配置及目录声明 | 5.074ms | 3.383ms |
| 系统环境 | 0.434ms | 0.188ms |
| 指令及 manifest | 5.364ms | 3.242ms |
| 渲染 | 0.205ms | 0.133ms |
| 用户附件 | 0.160ms | 0.139ms |
| 初始用量估算 | 15.016ms | 10.689ms |

对应 run ID：`98552941-bfcd-4d40-921f-1a8f10083e1c`、`824d1564-2c1a-49a7-b243-d7b380a7f0c3`、`e9fe9e11-08f7-4cef-b32e-4f08a2ef9610`。三次均收到预期短回复并完成；不等同于完整模型矩阵或所有思考档位验收通过。

脚本对照运行前后两条运行环境版本记录，文件名、长度和修改时间均未变化，确认没有因这些发送重新写入探测结果。

## 已确认的调用原因与后续边界

`deepagent-workspace/src/git.rs::detect_git_context` 在普通有提交的分支上串行启动四次 Git：仓库检查、分支查询、完整 porcelain 状态、最近提交。detached HEAD 还会增加一次短 hash 查询。这个入口由每轮系统上下文组装调用，不只首次启动时调用。

本次 Git 占两次预热上下文的约 88%～90%，是已定位的主要成本；项目结构扫描为第二项。总准备扣除上下文仍分别约 1322ms、1395ms，分散在工具/Hook 装配、持久化、请求 Surface 准备及计量等环节，不能把全部剩余时间归为 Git。

独立 PowerShell 对照的四次查询共约 373ms，低于应用内测量；执行环境和负载没有严格控制，不能替代原生数据，也不能据此断言差异来自单一系统因素。

天枢 `src/prompt/engine.ts` 的动态上下文按新用户消息生成，同一用户消息的工具续写复用。DeepSeek Harness 的 `ReactLoopAgent` 及 runtime context projection 也具有会话生命周期。本项目的工具、Hook、审批、取消和模型快照绑定本轮身份，不能直接缓存整个运行对象。

下一项应围绕 Git 查询数量和动态数据刷新边界设计回归，再处理项目结构与稳定工具声明的复用。不得通过永久缓存 Git 状态、跳过最新权限、删除指令或减少实际模型上下文来换取表面耗时下降。本次只补可验证的分项观测，没有擅自改变这些语义。

## 验证

- TDD RED：首次命令因 exact 名称过滤不匹配执行 0 项，不计为验证；随后运行完整测试名，真实执行 1 项并失败：预期一条分项日志，实际为 0。
- GREEN：完整测试名执行 1 项通过；验证八项计时、run 关联和路径/指令/提示词 canary 不进入分项日志。
- AppCore 相关回归：`context_runtime::` 58通过、2忽略；`system_context::` 11通过；`chat_service::tests::` 91通过、1忽略。合计160通过、0失败、3忽略，已包含上述新测试。
- 本轮四个 Rust 文件 `rustfmt --check --edition 2021` 通过；全仓 `cargo fmt --all -- --check` 仍因未修改的 `deepagent-core/src/id.rs` 失败。
- 原生 debug 构建通过；Responses 新会话、续聊、Messages 真实发送脚本通过，版本缓存记录保持不变。
- 本轮未重新执行完整 workspace 测试或 clippy；也未宣称 release 构建、受控低配置硬件基准、10ms 门限或完整双协议矩阵通过。

本地脱敏分析与测试日志位于 `target/chat-prep-audit-20261119/runtime-cache/`：`context-red-exact.log`、`context-green.log`、`context-regression.log`、`system-context-regression.log`、`chat-service-regression.log`、`context-native-build.log`、`context-native.json`、`context-native-context.json`。运行产物不提交。
