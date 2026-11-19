# 请求准备耗时：Git 上下文缓存与查询合并

日期：2026-11-19。

## 范围与结论

本轮只优化模型请求前的 Git 上下文读取，并接入系统内文件、Git、Shell 和 Hook 操作的失效通知。首次登录和基础系统设置约 90 秒不计入请求准备耗时；供应商响应等待也单独计算。

用户确认的规则：同一工作树复用 30 秒；系统内文件或 Git 操作后立即失效；外部修改最多复用至当前快照的 30 秒期限。

此前真实桌面对话中，Git 查询占 1353～2268 ms。包含目录链接修正的最终版本六个真实双协议场景全部完成：Git 缓存命中为 1.60～1.82 ms，保存或 TTL 失效后的读取为 548～786 ms。重启后的首次 Git 读取仍达 3007 ms，请求准备总耗时为 4816 ms；冷启动问题并未因减少进程数量而完全解决。续聊准备为 1136～1711 ms，说明 Git 并非全部剩余耗时的来源。

## 依据与实现

- 参考 `借鉴/Tianshu-harness/src/prompt/volatile-git.ts` 的 30 秒、有界缓存和并发查询复用机制。
- 参考 `借鉴/codex/codex-rs/git-utils/src/status.rs` 使用 porcelain v2 一次读取分支和状态的方式。
- 缓存的 Rust 同步、generation 失效、防止旧查询回填和统一失效守卫属于本地设计，用于适配现有同步上下文组装和多入口工具边界；相关行为由真实仓库及并发测试验证。

`deepagent-workspace` 持有唯一的进程内 Git 快照缓存，最多保留 50 个工作树。工作区子目录共享最近的 `.git` 工作树身份；30 秒从查询开始计时。失效时不接受正在执行的旧查询回填；查询失败的 `None` 也会过期。失效同时覆盖目录链接/符号链接的入口和实际目标，守卫保留逻辑路径，在操作后重新解析，避免删除或替换链接时漏掉所在工作树。缓存不承担文件权限判断，不记录工作区内容。

首次查询和失效后的查询从原来的 4～5 个 Git 进程合并为 2 个：

1. `git status --porcelain=v2 --branch -z --untracked-files=all`：读取分支、工作树状态和路径，同时识别是否为仓库。
2. `git log -3 --pretty=format:%h<US>%s`：读取近期提交；detached HEAD 复用首条提交的短哈希。

保留 NUL 分隔路径和已有的 10 个变更文件、3 条近期提交上限。新建仓库尚无提交时也能读取实际分支名。原有公开的 porcelain v1 解析器及桌面 Git 视图契约保持兼容。

## 失效入口

统一使用 `GitContextMutation` 在操作前后失效，错误、取消和 unwinding 也覆盖退出路径：

- Builtins：文件发布、父目录创建、删除、移动、Git commit、worktree 进入/退出、Shell。
- AppCore：原子写入、Office 文件生成、GitService 的修改操作、实际执行命令的 RuntimeCommandExecutor、ObservableHookRunner。
- Desktop：`save_text_file`、`save_binary_file`、`set_project_hooks_json`。

文件和明确的 Git 目标按路径失效。任意命令可能跨工作区，Shell、Hook、worktree 操作保守失效全部有界缓存。没有新增另一套执行链、Tauri command、协议字段或外部依赖；Builtins 只增加已有内部 workspace crate 依赖。

## 测试与门禁

证据位于忽略提交的 `target/chat-prep-audit-20261119/git-cache/`。

- 缓存 6 项初始 RED、跨工作区命令 1 项 RED，接入实现后转为 GREEN。
- 查询合并前，新建仓库分支测试真实返回 `(unknown)` 而失败；合并后通过。detached HEAD、子目录、中文重命名和未跟踪文件回归通过。
- 提交前额外用真实 Windows junction 复现了链接删除后只让外部目标失效的缺口，新增测试先 RED，修正入口/目标及逻辑路径保存后 GREEN。
- Workspace 当前 23 项单元测试串行通过；包含 TTL 边界、负缓存、工作树别名、目录链接删除、并发复用、查询期间失效、容量和全工作区失效。
- Builtins 写入→移动→删除、Shell 非零退出后的分支刷新、Git commit 刷新均经历 RED→GREEN；最终版本再次运行 integration target，并串行复跑，均为 3 通过。
- AppCore 原子写、Office、Hook、系统 manifest 缓存及 GitService 分支切换 5 项新增测试再次通过；最后一项目录链接失效修正在该轮之后，补跑 Workspace、Builtins 和原生验收覆盖修正。
- 最终 `cargo clippy -p deepagent-workspace --offline --all-targets -- -D warnings` 通过。
- 本轮 18 个 Rust 文件定向 rustfmt 检查通过。

整仓门禁未全绿，不能声称所有测试通过：

- Workspace 离线测试缺少已锁定的 `approx 0.5.1` 本地缓存。
- 整仓格式检查存在未修改的 `crates/deepagent-core/src/id.rs` 格式差异。
- Builtins 全量结果为 373 通过、3 失败、3 忽略。3 个失败都在创建符号链接测试样本时收到 Windows 1314（缺少特权），尚未进入被测文件操作。
- AppCore 首次全量结果为 1725 通过、12 失败、35 忽略。逐项复跑后 11 项通过；验证生命周期测试需取消继承共享 `CARGO_TARGET_DIR`，以使用各自 D 盘临时工程目录。剩余 `project_runtime_wins_over_managed_fallback` 在本机 PATH 存在 Node 24.18.1 时仍失败：解析器按现有 PreferLocal 规则返回 Local，测试期待 Managed。本轮未修改该解析规则。
- 常规 `pnpm build` 的脚本测试通过，但预打包技能目录时收到 Windows EPERM；单独 Vite 默认输出目录构建又遇到 `dist/file-viewer` 的 ENOTEMPTY。未清理或改动共享目录来绕过这些错误。`pnpm exec tsc --noEmit` 通过，改用全新隔离输出目录的 `pnpm exec vite build --outDir <D盘验收目录>` 成功。
- 最终 `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --bin deepagent-desktop -j 2` 成功。

## 真实原生验收

使用隔离验收数据，通过 Playwright CDP 脚本操作重编译的 Tauri 软件；未使用桌面控制插件。供应商沿用用户授权的 Layso，覆盖 Responses 与 Messages。原始请求、响应、密钥、数据库和日志不提交。

第一组缓存版本实测（未合并查询）：

| 场景 | 请求准备总耗时 | Git 上下文 |
| --- | ---: | ---: |
| Responses 重启后首次对话 | 2331 ms | 1233.600 ms |
| Responses 同会话续聊 | 1022 ms | 1.312 ms |
| Responses 系统保存文本后 | 1750 ms | 999.688 ms |

该脚本在下一次新对话按钮的原生坐标点击时被顶栏遮挡而停止；以上三次均已完成真实模型请求和终态。后续脚本使用同一个按钮的 DOM 激活进行后端耗时验收，不把这组数据作为界面点击性能证据。

最终验收使用隔离目录的生产前端资源，在真实 Tauri WebView 内通过 CDP 路由加载，保留原生 IPC 和真实供应商请求，避免开发服务器热更新打断。请求准备口径是 `run_requested → model_agent_transport_invoked`。另列进入传输层至首 Token 的等待，不将其混入请求准备时间，也不把它等同于供应商纯计算时间。

| 场景 | 请求准备总耗时 | Git 上下文 | 进入传输层至首 Token |
| --- | ---: | ---: | ---: |
| Responses 重启后首次对话 | 4816 ms | 3007.31 ms | 21134 ms |
| Responses 同会话续聊 | 1711 ms | 1.60 ms | 9732 ms |
| Responses 保存文本后 | 2199 ms | 686.43 ms | 4843 ms |
| Messages 保存二进制后 | 1803 ms | 786.00 ms | 3779 ms |
| Messages 同会话续聊 | 1136 ms | 1.82 ms | 4458 ms |
| Messages 等待 31 秒后 | 1550 ms | 548.41 ms | 2433 ms |

六次真实回复及完成终态均通过检查。两份运行环境探测缓存的文件名、字节数和修改时间在测试前后完全一致，支持本次验收没有重复执行版本探测的结论。中途另一次开发前端验收发生供应商连接失败并被热更新打断，未计为通过，也未用于最终性能对比。

最终证据：`runtime-cache/git-cache-alias-final.json`、`runtime-cache/git-cache-alias-final-context.json`；相关命令日志在本报告开头的 `git-cache/` 证据目录内。此前未包含目录链接边界修正的一组六场景也全部通过，记录在 `runtime-cache/git-cache-static-final.json`；报告表格采用最后一组数据，不挑选较快结果。

## 剩余性能工作

缓存命中不能消除全部请求准备成本。最终续聊仍为 1136～1711 ms，其中工作区结构扫描为 95～116 ms，Surface 准备为 88～99 ms，工具 schema 观察为 65～104 ms，请求计量为 33～47 ms，上下文用量发布为 46～48 ms。阶段存在包含关系，不能直接相加。冷启动 3007 ms 的 Git 阶段尚需继续区分进程启动、状态扫描及并发失效重查的贡献；本轮没有对应的进程级证据，不能推断唯一原因。当前证据不支持把剩余时间全部归为“资源初始化”，也不支持宣称整个系统达到 10 ms。

阶段诊断日志本身通过同步 `RuntimeLogStore::append` 写入，也存在测量开销。使用实际 Rust RuntimeLogStore 在同一 D 盘、隔离数据库内进行 10 次预热、100 次写入：平均 0.496 ms，中位数 0.401 ms，P95 为 1.048 ms；按 40 次估算约 19.8 ms。这个基准只覆盖日志队列和 SQLite append，不包含真实运行时的脱敏、事件处理及并发争用，不能拿来直接扣除实测总耗时。证据为 `git-cache/diagnostic-log-cost.log`。没有通过关闭权限、审批、取消、回放或必要持久化来缩短数字。

代码进一步确认，同一轮的 snip 提醒检查、主动压缩压力检查、上下文用量发布、请求指纹计量都会进入 `ModelAgent::context_measurement` / `measure_request`，包含请求准备、原生 Surface 计量及工具指纹计算。这是重复计算的明确调查方向，尚未做共享结果或变更其失效规则；不能把这些重复工作笼统称为“初始化资源”。

后续应按阶段进一步核对重复计算和同步持久化，保持权限、审批、取消和事件回放契约。完整双协议方案及模型矩阵不因本轮优化而标记完成。
