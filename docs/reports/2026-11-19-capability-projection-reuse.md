# 请求准备：复用运行内工具审计投影

## 范围与依据

承接运行环境持久化、Git 快照复用和请求计量优化。首次登录、向导及基础设置不计入请求准备；准备口径仍为 `run_requested → model_agent_transport_invoked`，传输层至首 Token 单列。

当前 `RunCapabilityRecorder` 在准入时已经冻结工具目录并计算初始投影，模型读取和协议审计仍会重复序列化、逐个哈希。同一工具续写的协议投影也重复计算。持久化层已经避免重复事件，但这不等于调用方避免了重复计算。

查证参考：`借鉴/deepseek-harness/packages/core/session/src/request-header.ts` 按工具顺序比较请求头，未变化时复用既有头；`借鉴/Tianshu-harness/src/prompt/engine.ts` 在同一用户消息的工具续写中复用上下文。本轮的有界、运行内审计投影复用为本地设计，不复制参考源码，也不改变本系统动态工具发现和每次发送的持久化校验。

## 修改与边界

- `crates/deepagent-app-core/src/run_capabilities.rs`：准入目录保留不可变工具定义和已经计算的哈希，后续精确比较定义后复用单工具哈希；初始模型投影随准入一起交给唯一 recorder。模型侧仅保留最近一组工具及投影，协议侧仅保留最近一组来源、模型、协议、实际 wire 工具和投影。
- 工具定义、顺序、可见集合、模型、协议或实际 wire 内容变化后重新投影；未知工具、重复工具或篡改定义仍拒绝。模型与协议哈希域、generation 和事件格式保持原契约。
- 协议侧的比较、计算和发布在同一运行内互斥边界中完成，并发相同输入只计算一次。只有持久化成功才更新本地状态。
- 相同输入命中时仍调用 `RunStore::record_capability_schema`，每次检查运行终态、snapshot 身份和独立写入者的 generation 冲突。复用计算不代表复用发送授权。
- 完整 schema 仅在当前 recorder 内存中保留；持久化仍只有哈希和路由元数据，不记录工具描述、请求正文或凭据。不新增 registry、run store、事件服务或依赖。
- `deepagent-models/client.rs`、`protocol.rs`：将既有待提交的工具名称、类型解析一并收敛到 codec 边界，保证三协议审计复用。Messages 普通工具按自身 schema 识别为 custom，排序后的工具按名称关联；不修改 wire 内容。选择性提交只包含这部分审计访问器，思考参数编码和供应商缓存策略继续保留在工作树。
- `crates/deepagent-app-core/src/run_capabilities_reuse_tests.rs`：新增行为回归及显式执行的合成基准；计数器仅在测试编译中启用。

## TDD 与局部证据

证据位于忽略目录 `target/chat-prep-audit-20261119/schema-reuse/`。

初始行为 RED：相同初始模型 schema 三次读取发生 9 次哈希；相同 Responses wire 再读发生 7 次哈希；8 个并发调用分别重复计算。原有 12 项审计回归通过，新三项均失败。首次计数器可见性编译错误不计为行为 RED。

修改前合成基准使用 50 个工具，model/wire 各重复调用实际 recorder 100 次，保留真实内存 RunStore 检查，不发送模型请求、不执行工具，也不用时间阈值断言：

| 版本 | 模型侧耗时 | 模型侧哈希次数 | 协议侧耗时 | 协议侧哈希次数 |
| --- | ---: | ---: | ---: | ---: |
| 修改前 | 1,207,234 µs | 5,100 | 3,032,829 µs | 10,300 |
| 修改后 | 105,134 µs | 0 | 160,002 µs | 52 |

基准记录为 `probe-before.log`、`probe-after.log`。模型侧局部耗时约减少 91%，协议侧约减少 95%；协议侧的 52 次哈希来自首次 50 个工具及两个整体哈希，其后 99 次只复用本地计算。每次真实 RunStore 校验仍执行，局部基准不能直接外推为一次请求或整体交互的改进。

## 验证状态

- 最终相关集：18 通过、1 个显式基准忽略；原来的三个 RED 均转 GREEN。新增边界测试覆盖三协议、完整定义篡改、工具顺序、模型、wire 内容、并发、终态、数据库读取失败后重试和独立写入者冲突。内部数据堆分配调整后的最终复核同样为 18 通过、1 忽略，对应 `green-boxed.log`。
- 本次 AppCore 完整库命令为 1742 通过、1 失败、36 忽略。失败在 `runtime_service::tests::project_runtime_wins_over_managed_fallback:3168`：本机 PATH 已有 Node 24.18.1，符合该用例的 Node 24 要求，程序按原有本地优先规则返回 Local，而测试预期 Managed。原环境独立复跑仍失败；只在独立测试进程排除宿主 Node 路径后通过，没有修改机器环境或生产运行时逻辑。
- 首次在完整测试运行期间重编译同一个 exe，被 Windows 文件占用阻止，LNK1104。后续改为串行。该次链接失败单独记录，不计为通过，也不作为行为缺陷。
- 本轮修改的 Rust 文件定向 rustfmt 通过。整仓格式检查仍报告未修改的 `deepagent-core/src/id.rs` 四处宏排版差异。
- `cargo test -p deepagent-persistence --offline --lib -j 2 -- --test-threads=1`：226 项通过，包括持久化、终态和独立写入者竞争。
- 严格 Clippy 首次检查发现缓存数据使准入 enum 变大，本轮已将内部缓存数据改为堆分配，保持调用边界；最终相同命令复核确认该问题消除。整组仍因 `canvas_artifact_service.rs:835`、`model_runtime.rs:180`、`sandboxie_service.rs:844`、`system_context.rs:104/108`、`input_runtime.rs:1170`、`system_prompt.rs:303/304` 的 8 个既有 lint 失败，未新增豁免或顺手改动这些文件。日志为 `clippy-final.log`。
- 全仓离线检查在补齐 `approx 0.5.1` 后继续发现 `atomic 0.6.1` 缓存缺失；已查证 Cargo 参数并执行 `cargo fetch --locked --target x86_64-pc-windows-msvc`，依赖仅进入 D 盘 CARGO_HOME，锁文件未变。
- `cargo test --workspace --offline --no-run -j 2` 最终退出 101：`deepagent-builtins/src/file_mutation.rs:113` 的 `ToolOutput::success(output)?` 不符合闭包返回类型，`deepagent-app-core/src/service.rs:1009` 编译时找不到 `RunStore::tool_completions`。这些文件正由其他会话修改，本轮没有改动；随后源码已出现无 `?` 的调用及该方法定义，说明编译期间存在源码变动，不能把这次结果当成当前版本全绿或直接判定为固定缺陷。原始失败保留在 `workspace-no-run-final.log`。

## 原生验收的版本边界

共享工作区第一次原生构建因其他会话尚在修改的 `file_mutation.rs:117` 返回类型不一致退出 101，日志为 `native-build.log`。本轮没有修改该文件。为核验选择性提交的独立性，从 `e79cdfe0` 加六个候选文件生成忽略目录内的源码快照，使用既有桌面锁文件及资源输入执行 `cargo build --locked --offline --bin deepagent-desktop -j 2`，构建通过，对应 `native-snapshot-build.log`。未新增依赖或修改仓库锁文件。

复制旧构建资源时，Windows 防护拒绝读取一份安全研究参考文档。没有关闭或绕过防护；验收改用当前桌面项目资源输入集，未复制被拦截文件。

独立提交快照在真实 Tauri WebView 中通过 Responses 的重启首次、续聊、系统保存后和工具续写四场景，均完成且回复 `OK`。工具续写有两次完成的工具调用及三次模型请求，持久化只有一份 wire 快照。Messages 四场景均在本地编码时因 `Messages system items must be supplied as instructions` 失败；修正尚在工作区协议代码中，未包含在本次独立提交。不能把这组结果表述为双协议全部通过。

独立源码快照另运行 `cargo test --locked --offline -p deepagent-app-core --lib run_capabilities::tests -j 2 -- --test-threads=1`，18 通过、1 个显式基准忽略，日志为 `snapshot-schema-tests.log`。该结果验证六文件选择性提交不依赖工作区其他未提交实现才能通过相关回归。

这份快照也没有 AppCore 尚未提交的准备阶段 observer 接线，因此没有 `run_preparation_phase` 数据，不能用缺失计时推导零耗时。原始八场景和事件统计保存在 `runtime-cache/snapshot-schema-final.json` 与 `schema-reuse/snapshot-native-events.json`。后续当前集成工作树验收单独记录，保留版本差异。

## 当前集成工作树的真实结果

其他会话修正编译问题后，`cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --locked --offline --bin deepagent-desktop -j 2` 通过，日志为 `native-integrated-build.log`。运行原生程序、原生 IPC 和先前生产静态前端，通过 Playwright/CDP 操作，未使用桌面控制插件。本轮没有修改 UI 或 Tauri 桥接，因此没有重复构建当前其他会话修改的前端。

`verify-schema-reuse.cjs` 八场景全部完成且回复 `OK`。两个运行环境缓存文件的大小和修改时间均未变化；两个工具续写场景都核对到成功读取指定 fixture 的 `read_file`，随后再次调用模型。Responses 同一场景还执行了一次 `skill`；没有将普通回复冒充工具验收。原始结果为 `runtime-cache/schema-final.json`，阶段结果为 `runtime-cache/schema-final-context.json` 与 `schema-reuse/native-schema-phases.json`。

单位为毫秒，首次登录、向导及基础设置均不包含在内：

| 场景 | 请求准备 | Git 上下文 | 首次 wire 审计 | 传输调用至首 Token |
| --- | ---: | ---: | ---: | ---: |
| Responses 重启首次 | 3505 | 2272.641 | 85 | 9367 |
| Responses 续聊 | 1647 | 1.104 | 105 | 28411 |
| Responses 系统保存文本后 | 3022 | 1047.470 | 132 | 22999 |
| Messages 系统保存二进制后 | 1794 | 850.878 | 59 | 9162 |
| Messages 续聊 | 1236 | 0.653 | 100 | 24661 |
| Messages 等待 31 秒后 | 1503 | 651.762 | 55 | 6447 |
| Responses 工具续写 | 1252 | 0.747 | 76 | 19706 |
| Messages 工具续写 | 1858 | 741.241 | 56 | 9212 |

两个工具续写场景都实际调用模型两次，wire 审计分别为 `76 → 22 ms` 与 `56 → 17 ms`，每个 run 都只有一份 wire 快照；模型初始 schema 在准入 `capability.snapshot` 内，没有重复生成独立的 model schema 事件。首次模型 state refresh 为 10～19 ms，续写为 10～11 ms，仍经过持久化校验。

“传输调用至首 Token”包括适配器编码、连接和服务端处理，当前没有进一步拆分，不能全归因给供应商。测试期间仍有其他编译进程，一次系统 CPU 采样为 96%；记录在 `native-load-sample.json`、`native-process-load.json`。这是集成行为和当时性能记录，不是同负载的端到端前后对照。相较前轮 760/918 ms 的续聊，本次总体更慢，不能声称这轮使整体延迟下降。

## 剩余问题

- Git 规则继续保持用户确认的 30 秒复用、系统内文件或 Git 操作立即失效；缓存命中约 0.65～1.10 ms，首次、失效或过期后的真实查询仍有 0.65～2.27 秒。
- 工作区结构扫描为 130～262 ms；会话绑定至 registry 就绪仍为 228～591 ms，这段尚需继续细分，不能直接归因于凭据或工具构造。Snip 工具挂接还有 36～176 ms。
- 此次 Hook 派发和 ModelAgent 构造记录均为 0 ms，运行环境投影约 0.17～0.88 ms（首次 0.50 ms）；没有证据支持“每轮重新探测运行环境或初始化 Hook”是当前秒级等待来源。
- 首次 wire 审计仍有 55～132 ms；本轮复用消除后续重复计算，不取消每次发送的数据库屏障，剩余持久化成本尚需定位。
- 整体尚未达到 10 ms，未完成同负载端到端前后对照、完整思考参数模型矩阵及其余 Harness 验收。本轮可独立提交的实现与当前工作区集成结果有明确版本边界，完整目标继续进行。
