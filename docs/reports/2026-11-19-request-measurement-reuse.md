# 请求准备：复用相同请求的本地计量

## 范围与根因

本轮承接运行环境持久化与 Git 上下文缓存。首次登录、向导和基础设置不计入发送耗时；请求准备仍按 `run_requested → model_agent_transport_invoked` 统计，传输层至首 Token 单列。

`ModelAgent` 的 snip 提醒、prefire、主动压缩检查、用量发布及发送前计量都会调用 `context_measurement` / `measure_request`。相同请求反复克隆、投影、序列化工具和离线计量，属于重复计算，不能统称为每轮初始化资源。已有 DeepSeek tokenizer 通过 `OnceLock` 复用，不能据此声称 tokenizer 每轮重载。

参考核对：

- `借鉴/deepseek-harness/packages/llm/llm/src/index.ts`：`prepareCall` 绑定 adapter 注册、解析并冻结本次请求配置。
- `借鉴/deepseek-harness/packages/core/agent-loop/src/agent.ts`：请求构建使用 Surface generation，并在请求头变化时记录新头。
- `借鉴/Tianshu-harness/src/prompt/engine.ts`：同一用户消息的工具续写复用 fresh context，新用户消息重建。

本轮没有复制参考项目源码。以下单条计量复用为本地设计：不改变配置、Skill、Hook 的刷新时机，也不缓存绑定 run/session、审批、权限或取消状态的运行对象。

## 修改与失效边界

- `crates/deepagent-runtime/src/model_agent.rs`：单个 ModelAgent 保存一条本地计量结果，以现有 `ResponseRequest::PartialEq` 精确比较完整准备后请求；不以模型名、消息数或有碰撞可能的短哈希代替输入。线程并发查询同一个请求也只计算一次。
- 输入、指令、工具 schema、模型、思考强度、输出上限、采样、工具选择及输出格式变化时重新计量；替换、snip、压缩和工具发现自然进入同一边界。回到旧请求也会重新计算，没有无界历史缓存。
- 每次查询仍先走图片能力门禁及 resolver 的可用性检查；附件预算变化时重新计量。不因复用结果跳过最终请求或图片完整性校验。
- 与既有 `prompt_pricing_body` 一致，只为计量归一化 stream 和临时图片 URL / Files ID；不在缓存中保留图片字节。正文快照仅保留在当前代理内存，不落盘、不写日志。
- 只复用本地投影和请求指纹；供应商用量 anchor、压缩阈值及 UI 用量校准仍读取实时状态。
- 细分诊断复用 `with_preparation_observer`，新增 `model_agent_context_measurement_computed` 表示实际重新计算。内容、路径、工具参数和凭据均不进入该诊断。
- `events.rs`、`kernel.rs`、`loop_engine.rs`：补齐既有准备阶段诊断的提交边界，三个层级共用 `PreparationObserver` 类型，覆盖 Hook 执行、首轮进入及必要持久化阶段。诊断不改变既有事件或审批处理。
- `crates/deepagent-runtime/src/model_measurement_reuse_tests.rs`：回归覆盖重复查询、完整请求依赖、图片预算、三协议投影、临时图片数据及并发；另有显式执行的合成计量基准，不使用时间断言。

## TDD 与局部性能证据

证据保存在忽略目录 `target/chat-prep-audit-20261119/`，不提交数据库、模型原始请求、响应或日志。

初始三个行为测试均 RED：压力检查、UI 用量和发送前实际计算三次；相同请求重复查询和图片计量也重复执行。接入复用后，三个测试均 GREEN。另补三协议、并发和图片数据边界。

相同本地合成负载（48 个工具和合成历史，100 次重复查询）：

| 版本 | 计时的重复查询耗时 | 实际计算次数（含一次预热） |
| --- | ---: | ---: |
| 修改前 | 1,460,893 µs | 101 |
| 修改后 | 157,731 µs | 1 |

对应 `measurement-probe-before.log`、`measurement-probe-after.log`。局部耗时约减少 89%，不能解释为端到端发送也减少 89%，也不能据此宣称交互达到 10 ms。

## 验证结果

- Context 全量 lib：71 通过。
- Runtime 全量 lib 串行：501 通过、1 失败、3 忽略。失败为未修改的 `checkpoint::access::native::tests::restrictive_tree_is_protected_before_copying_any_bytes`，在恢复临时样本 DACL 时报告 `restored test DACL differs`；独立重跑仍失败，不能宣称整组全绿。
- 收敛诊断回调类型后，最终 Runtime 串行回归：501 通过、3 忽略，显式排除上述已单独复核的 1 项 ACL 用例。计量六项回归、Hook/首轮边界诊断，以及既有重试、压缩、校准、取消、终态和持久化测试均包含在通过范围。
- `cargo clippy -p deepagent-runtime -p deepagent-context --no-deps --offline --all-targets -j 2 -- -D warnings` 最终通过。包含依赖 lint 的原命令受未修改的 `deepagent-persistence/src/document_store.rs:280` 的 `chunks_exact_to_as_chunks` 阻断；未修改无关代码或增加 lint 豁免。
- 修改的 Rust 文件定向 rustfmt 通过。整仓 `cargo fmt --all -- --check` 仍报告未修改的 `deepagent-core/src/id.rs` 四处宏排版差异。
- `cargo test --workspace --offline -j 2 --no-run` 本轮实际执行，因缺少已锁定的 `approx 0.5.1` 缓存而不能启动全仓测试。
- 最终 `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --bin deepagent-desktop -j 2` 成功，用时 2 分 29 秒。

本轮无前端或 Tauri 消息结构变更，没有重新运行前端构建。原生验收沿用上一轮生产前端静态资源，通过 CDP 路由加载进真实 WebView，原生 IPC 和供应商请求保留。每条对话按本 run 的 `run_completed` 回复和完成通知核验，不使用页面旧消息中的 `OK` 作为成功依据。

## 最终原生验收

使用已经完成首次登录、基础设置的隔离数据，重启最终程序，通过 Playwright/CDP 操作模型胶囊和发送按钮。供应商沿用用户已授权的 Layso；Responses 为 `gpt-5.6-sol`，Messages 为 `claude-opus-5-5`。

| 场景 | 请求准备 | Git 阶段 | 压力检查 | 用量发布 | 发送前计量 | 传输层至首 Token |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Responses 重启首次 | 1404 ms | 554.56 ms | 5 ms | 8 ms | 3 ms | 6645 ms |
| Responses 续聊 | 760 ms | 0.49 ms | 7 ms | 6 ms | 2 ms | 4645 ms |
| Responses 系统保存文本后 | 1712 ms | 816.75 ms | 5 ms | 5 ms | 2 ms | 2477 ms |
| Messages 系统保存二进制后 | 1646 ms | 759.35 ms | 5 ms | 11 ms | 3 ms | 1948 ms |
| Messages 续聊 | 918 ms | 0.65 ms | 7 ms | 13 ms | 4 ms | 3827 ms |
| Messages 等待 31 秒后 | 1630 ms | 744.15 ms | 7 ms | 12 ms | 3 ms | 2504 ms |

六次均收到当前 run 的预期回复 `OK` 和 `completed` 终态。两份运行环境缓存文件的名称、字节数和修改时间前后一致。Git 命中、系统保存后失效及 TTL 过期行为均保留。原始证据：`runtime-cache/measurement-final.json`、`runtime-cache/measurement-final-context.json`，命令输出为 `measurement-native-audit.log`。

上一轮发送前计量为 33～47 ms，本轮为 2～4 ms。续聊准备从上一轮 1711/1136 ms 到本轮 760/918 ms，属于两次完整运行的观察对比，不能全部归因于此次复用：例如 Responses 的 runtime snapshot 持久化从上一轮 566 ms 到本轮 1 ms，本轮没有修改该逻辑。阶段存在包含关系，不应将它们直接相加。

续聊仍有上下文组装 109/148 ms、schema source 刷新 26/33 ms、发送前工具 schema 审计 72/70 ms，以及用户输入、会话状态和其他必要持久化。构造 ModelAgent 为 0 ms，Hook 实测阶段为 0 ms；这些样本不支持把剩余时间解释为每轮重新初始化资源。仍需继续分解未覆盖间隔、工作区扫描和审计投影，不能宣称整个软件达到 10 ms，也不能把传输层至首 Token 等同于供应商纯计算时间。
