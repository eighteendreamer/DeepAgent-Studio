# 请求准备：目录扫描与工具准备分段

## 范围

承接 H032，继续定位请求发出前的系统内部耗时。首次登录、向导和基础设置不计入；请求准备仍以 `run_requested → model_agent_transport_invoked` 为口径，传输调用至首 Token 单列。

本轮只修改工作区扫描和既有工具准备链路的诊断，不调整思考策略、供应商、远程会话、审批或沙箱权限。Git 快照沿用已确认的规则：复用 30 秒，系统内文件或 Git 操作立即失效，外部修改最多滞后 30 秒。

## 扫描根因与修复

`WorkspaceScanner::walk` 原来收集路径，在排序比较器中反复调用 `Path::is_dir()`，输出时再查询一次。目录分类因此随排序比较次数增长，属于同一次扫描内的重复文件系统查询。

现在收集 `(PathBuf, bool)`，每个条目只查询一次，排序和输出复用结果。仍使用原来的 `Path::is_dir()`，保留目录链接跟随、目录优先与文件名排序、忽略目录、深度和条目上限；不引入跨扫描缓存。下一轮扫描仍能看到文件变目录及 manifest 更新。

这是基于现有扫描边界的本地实现调整，不引入新服务、依赖、公共 API 或持久化字段。测试计数器仅在测试编译中存在。

## TDD 与局部基准

证据位于忽略目录 `target/chat-prep-audit-20261119/entry-scan/`。

- `red.log`：旧实现对 50 个目录条目做了 214 次目录分类，新回归要求 50 次；7 通过、1 失败。这是实际行为 RED。
- `green.log`：完整 workspace crate 库测试 25 通过、1 个显式基准忽略。覆盖排序输出、文件/目录数量、忽略目录、截断边界，以及下一次扫描观察到文件类型与 manifest 变化。
- `probe-before.log`、`probe-after.log`：每种输入执行 20 次真实扫描与上下文渲染，不发送模型请求、不读取凭据，无时间阈值断言。

| 输入 | 修改前目录查询次数 | 修改后目录查询次数 | 修改前累计 µs | 修改后累计 µs | 两版渲染字节数 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 合成宽目录 | 21880 | 9280 | 334825 | 180413 | 55260 |
| 当前仓库 | 28200 | 5380 | 388222 | 117608 | 121060 |

目录查询量分别减少约 58% / 81%，局部耗时减少约 46% / 70%。仓库单次平均约从 19.4 ms 降为 5.9 ms。这是较空闲时的局部对照，不能与 H032 高负载原生扫描的 130～262 ms 直接对比，也不能推出整个请求或 UI 已达到 10 ms。

## 工具准备计时设计

沿用 `build_run_context` 已有的内存累计、末尾一次记录模式，避免每个子阶段单独同步写日志扰动测量。`prepare_chat_tools` 的诊断只有静态阶段名与微秒；模型绑定耗时附加到既有 `registry_ready` 事件。不得记录 schema、Skill 正文、提示词、路径、凭据或请求响应正文。

工具准备可能发生在 SDK 准入阶段。计时应在实际准备时记录，消费冻结 registry 时不能重复生成，也不能将准入前耗时误加到后续执行区间。

`tools-profile-red.log`：聊天完成后未找到诊断记录，实际断言为 0 条、预期 1 条。`tools-profile-green.log`：新增工具准备与原有上下文准备两项回归通过；用现有 ChatService 跨实际执行路径验证，覆盖普通聊天、准入时记录、消费时无第二条诊断及敏感内容不出现在计时负载中。

## 验证与原生结果

已经通过扫描器严格 Clippy；本轮四个 Rust 文件定向 rustfmt 与 diff 检查通过。整仓 `cargo fmt --all -- --check` 仍失败于未修改的 `deepagent-core/src/id.rs` 四处宏排版差异，日志为 `fmt-workspace.log`。

`cargo clippy -p deepagent-app-core -p deepagent-workspace --offline --all-targets --no-deps -j 2 -- -D warnings` 仍被 H032 同样的 8 项 lint 阻断，涉及 `canvas_artifact_service.rs:835`、`model_runtime.rs:180`、`sandboxie_service.rs:844`、`system_context.rs:104/108`、`input_runtime.rs:1170` 和 `system_prompt.rs:303/304`。本轮未修改这些代码，未增加 lint 豁免；日志为 `clippy-app-core.log`。

AppCore 完整库测试使用本轮编译出的测试二进制执行，结果为 1743 通过、2 失败、36 忽略，日志为 `app-core-full.log`。一项仍为本机 PATH 中 Node 24 满足要求、生产选择 Local 而旧断言期望 Managed；另一项为验证生命周期夹具编译等待共享构建目录，120 秒超时。取消继承 `CARGO_TARGET_DIR`、使用夹具自己的 D 盘目标目录单独复跑，生命周期用例通过，日志为 `verification-lifecycle-recheck.log`。没有修改这些生产实现或测试断言，原完整命令仍不能记为全绿。

追加 `cargo test --workspace --offline -j 2 -- --test-threads=1` 已收取终态（退出 101）。Workspace feature 合并后重建依赖，运行到 AppCore 库测试为 1744 通过、1 失败、45 忽略；唯一失败仍是上述宿主 Node 优先级断言，验证生命周期用例本次通过。Cargo 随即停止，后续 crate/integration/doc 测试未继续，不能称为全仓全部已执行。该命令编译在 Skill 快照改动前完成；后续 Skill 快照改动有单独的定向验证，不混用这次结果。日志为 `workspace-tests.log`。

`cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --locked --offline --bin deepagent-desktop -j 2` 成功，日志为 `native-build.log`。此前已清理的编译缓存导致依赖重建，该次耗时 18 分 14 秒，属于开发构建，不是软件发送消息耗时。

使用已登录的 D 盘隔离数据，Playwright/CDP 脚本操作真实 Tauri，保留原生 IPC 与用户授权的 Layso 请求，未使用桌面控制插件。本轮无 UI 或桥接契约改动，前端采用此前生产静态资源；未声称验收当前其他会话的前端改动。构建与全量测试结束后才开始四场景，开始时 CPU 采样为 32%，没有已知 Cargo/rustc 活跃进程。

四次均完成且回复预期 `OK`，两份运行环境缓存的大小和修改时间均未变。结果为 `entry-scan-final.json`、`entry-scan-final-preparation.json`、`native-verification.log`。以下单位为毫秒：

| 场景 | 系统准备 | Git | 目录扫描 | 工具构建 | 能力元数据 | 能力快照 | 模型绑定 | 传输至首 Token |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Responses 重启后首次 | 2439 | 1606.576 | 127.531 | 100.227 | 32.935 | 73.204 | 2.691 | 6823 |
| Responses 续聊 | 929 | 0.828 | 33.545 | 77.903 | 36.222 | 85.760 | 29.009 | 7619 |
| Messages 首次 | 1310 | 633.165 | 31.366 | 65.187 | 45.598 | 105.998 | 1.307 | 5415 |
| Messages 续聊 | 1050 | 8.995 | 41.796 | 100.146 | 104.501 | 120.455 | 4.646 | 6901 |

Messages 首次是该进程中的新对话，不是再次重启。期间 Git 30 秒 TTL 已过，不把重新查询说成缓存失效缺陷。四次工具准备都发生在 `run_requested` 后，且每轮只有一条工具 profile；模型绑定不是这组数据中的主要慢点。

脚本发起发送到 `run_requested` 另为 492 / 305 / 549 / 320 ms，包括 Playwright 点击准备、前端和 IPC，尚未进一步拆开，不能全部归因于 UI 或后端准入，也不能把后端准备时间当成整个点击响应。传输至首 Token 包含适配器、连接和服务端，不能全部称为模型纯计算时间。

首次 Git 真查询仍占约 1.61 秒。缓存命中的续聊还存在工具注册、Skill/插件/配置哈希、能力快照、用户输入持久化、Hook/审批组装和运行内核准备等成本。Responses 续聊时间线保存在 `warm-preparation-timeline.json`，其中工具准备五段累计约 211 ms、目录扫描约 34 ms、Snip 投影约 55 ms、首次 wire 审计约 51 ms，另外还有生命周期与日志成本；父子阶段存在包含关系，不能把所有计时直接相加。

本轮较上一轮总体更快，但运行负载不同，未做相同负载端到端 A/B，不能将差异全部归因于扫描修复。系统准备仍为 0.93～2.44 秒，未达到 10 ms。运行环境投影约 0.43～5.64 ms，且磁盘缓存未改写，当前没有每轮重新做 Node/Python/Java 初始化的证据。完整思考模型矩阵及其余 Harness 验收继续进行。
