# 模型列表滚动、Slash 模型路由与聊天延迟检查

日期：2026-11-18。

## 修复边界

- `SettingsView.tsx`、`ModelsSettings.tsx`：模型页使用设置视口的可用高度，固定页头和表头。供应商列与模型列表分别拥有滚动容器，并限制滚动链；其他设置分类保留原滚动方式。
- `App.tsx`：聊天主视图随 App 加载，消除首次发送时加载 ChatView 模块导致整个主区域进入 Suspense 的路径。用户截图中的浮动消息和中央 loading 同时出现，与该路径一致。
- `Composer.tsx`、`useComposerModelSettings.ts`：`/model` 只从启用供应商下的已启用模型生成候选项，显示实际供应商名称。胶囊为展示而保留的旧选择不混入候选目录；候选命令携带供应商身份。
- `ComposerSuggestPanel.tsx`：鼠标悬停更新高亮时不强制滚动；键盘上下选择或搜索结果变化才将选中项移入可见范围。原实现把悬停和键盘选中共用同一滚动副作用，滚轮经过鼠标所在行会再次触发滚动。
- `slash_runtime.rs`、`harness_model_service.rs`、`settings/harness_registry.rs`：Slash 与模型胶囊使用同一注册表选择入口，移除该命令对旧 discovered set 的依赖。支持唯一模型名或 `provider_id/model_name`；重名时明确要求指定供应商，未启用模型拒绝选择。当前 turn 的模型快照和下一轮生效规则不变。

这是现有 UI 生命周期和注册表边界内的本地设计，没有新增模型协议、远程调用入口或权限规则。

## 历史两分十五秒请求的证据

使用只读 SQLite 脚本 `apps/desktop/scripts/audit-run-latency.py` 读取对应运行日志。脚本只导出事件时间和限定的元数据字段，不导出消息、工具参数或凭据。以下时间相对 `run_requested`，不包含该标记之前的 UI/IPC 准入耗时。

| 阶段 | 相对时间 / 耗时 |
| --- | ---: |
| 工具注册表准备完成 | 446ms |
| hooks 注册完成 | 1,131ms |
| 上下文快照提交 | 2,815ms |
| 第一次模型请求发出 | 3,746ms |
| 流空闲超时，自动重试标记 | 55,756ms |
| 模型首轮完成，已包含重试 | 89,994ms |
| skill 执行 | 工具报告 6ms |
| 工具结果后的第二次模型请求 | 91,319ms |
| 第一个正文 token | 133,397ms |
| 整轮结束 | 134,777ms |

结论：大部分时间消耗在模型流等待、一次流空闲超时重试，以及工具结果后的第二次模型调用。skill 本身没有运行两分钟。本地准备仍有约 3.75 秒，不能把整轮延迟全部归给外部服务，也不能仅凭日志断言是电脑占用满造成的。该次重试原因是 `model SSE idle timeout`，不同于用户要求立即报错、零重试的过载错误；本轮没有擅自修改重试策略。

## 脚本与回归验证

`apps/desktop/scripts/check-chat-desktop.cjs` 通过 CDP 直接连接重新编译的 Tauri WebView2，检查原生 Tauri 标记并调用真实 IPC，不使用浏览器 API mock 或桌面控制插件。记录事件类型、DOM 准备时间和截图，不导出 IPC 参数与聊天正文；截图作为本机验收资料保留在忽略目录。当前 Tauri 的 invoke 属性不可写，最终脚本没有尝试覆写或宣称测得全部 IPC 耗时；后端阶段时间使用实际运行日志。

测试副本来自现有数据库的 SQLite backup；会话文件和插件目录复制到隔离数据根。额外加入 24 个停用供应商、100 个未启用模型，验证长列表滚动与候选项过滤。没有向原始用户数据库加入压力数据。插件复制时 Windows 长路径导致两个 `.nyc_output` 覆盖率文件未复制，测试未把副本视为插件目录的完整性验证。

脚本环境项：`DESKTOP_AUDIT_CDP` 指定已启动原生进程的 CDP 地址，`DESKTOP_AUDIT_OUTPUT` 指定结果目录，`DESKTOP_AUDIT_CPU_RATE` 指定 WebView CPU 减速倍数；`DESKTOP_AUDIT_GREETING=1` 额外发送一次真实“你好”，会调用当前配置的模型并产生供应商用量。默认只发送本地 `/model` 命令。`DESKTOP_AUDIT_PROFILE=1` 额外保存切换附近的 CPU profile，诊断采样与常规计时分开。

- `pnpm build`：完整前端回归、TypeScript 与 Vite 构建通过；保留既有大 chunk 和文件预览依赖提示。
- `pnpm exec tsc --noEmit`：通过。
- `node --test scripts/check-composer-controls.test.cjs`：30 项通过，包含旧展示选择不能混入 Slash 启用目录。
- `cargo test -p deepagent-app-core --test harness_registry_run --test harness_model_service --offline -j 1`：31 项通过。新增回归覆盖 Slash 列表过滤、选择不发推理请求、下一轮真实请求路由、同名歧义与未启用拒绝。
- 本轮五个 Rust 文件（含聊天单元测试）的 `rustfmt --check`：通过。
- `cargo fmt --all -- --check`：未通过，差异仅位于本轮未修改的 `crates/deepagent-core/src/id.rs` 四处宏调用；没有顺手改格式。
- `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --features custom-protocol --offline -j 1`：通过。实际启动重新编译的原生进程，前端使用打包后的生产资源，Rust 为 debug 构建；验收没有依赖 Vite 开发服务器。
- `cargo test --workspace --lib --offline -j 1`：未通过，在 app-core 库测试阶段停止，结果为 1,704 通过、17 失败、44 忽略；后续 workspace 库未获得完整执行结果。其中 `/model` 的旧测试要求列出仅由发现接口返回、并未启用的 Pro 模型，且断言旧用法文案 `<model_id>`。本轮已按真实初始化行为更新测试：显示注册表中启用的 Flash、排除仅发现的 Pro，使用 `<model_name>` 用法；生产行为未因此调整。其他失败涉及媒体大小边界、hooks/压缩上下文、运行时选择和验证命令生命周期；部分日志为超时，尚未逐项确定原因，不能笼统归为机器负载或宣称全库通过。原始结果保存在 `target/chat-ui-audit-20261118/workspace-lib.log`。
- 串行复测 `cargo test -p deepagent-app-core --lib chat_service::tests:: --offline -j 1 -- --test-threads=1`：89 通过、1 忽略、1 个旧 Slash 断言失败。全库执行时失败的两个聊天 hooks 测试本次均通过；仅凭复测不能断言它们在所有负载条件下稳定。日志为 `target/chat-ui-audit-20261118/chat-service-regression.log`。
- 更新上述旧断言后，`cargo test -p deepagent-app-core --lib chat_service::tests::slash_model_without_args_lists_only_activated_registry_models --offline -j 1 -- --exact`：1 项通过、0 失败。构建保留 `LNK4098` 默认库冲突警告，未阻止测试运行。日志为 `target/chat-ui-audit-20261118/slash-unit-regression.log`。最后仅修改测试断言，没有改变已完成原生验收的生产代码。
- 延迟导出脚本经 Python 语法检查及真实只读数据库运行验证，复核本轮请求为 23,967ms、52 个事件。重试原因仅导出固定分类，避免输出任意上游错误正文。桌面脚本经 `node --check` 通过。

本轮未运行全量 clippy 和完整 workspace integration suite；已执行上列定向集成测试、前端回归、原生构建和桌面脚本验收。

## 实际桌面验收结果

原生脚本常规 1x、4x WebView CPU 减速、无供应商前缀命令复测均通过；捕获的页面异常为 0，模型选择在测试结束时恢复。4x 只减速 WebView，不能当作整台低配置电脑或所有性能上限的证明。

| 场景 | 1x | 4x |
| --- | ---: | ---: |
| Start → Chat 的 DOM 准备 | 109.5ms | 325.3ms |
| 整个主区域的 Suspense loading | 未出现 | 未出现 |
| Slash 连续滚轮回弹 | 未出现 | 未出现 |
| 模型和供应商内部滚动 | 通过 | 通过 |
| 1280×800、960×600 外层溢出 | 无 | 无 |

另外直接发送 `/model gpt-6-astra`，未附带供应商前缀，成功持久化选择，耗时样本 114.2ms。候选项与真实启用目录的 10 个模型一致，不包含额外注入的 100 个未启用模型。后端集成回归另覆盖了未启用供应商与模型同名的情况。

在 1280×800 下模型滚动视口高 419px，内容高 5,777px；供应商视口约 635px，内容高 1,844px。滚轮分别令两列 scrollTop 增加 800px，外层始终为 0。在 960×600 下模型视口高 219px，两列仍能独立滚动，外层 scrollHeight 与 clientHeight 相等。左侧设置分类导航保留其自身滚动，与供应商列分开。

新的真实“你好”使用原来保存的 `gpt-6.2-sol` 和高思考档位，UI 整轮约 24.4s。唯一事件源的后端日志为 23.967s，起点是 `run_requested`，两者不混用：注册表 589ms、hooks 1,302ms、上下文快照 2,879ms、首次请求 3,313ms、skill 工具报告不足 1ms、第二次请求 16,137ms、首个正文 token 22,995ms。本次没有流空闲超时重试。供应商响应存在波动，不能把 134.8s → 24.4s 宣称为本轮代码的性能提升比例。

本地准备并非零成本：现有同步路径会准备权限与工具、构造 hooks 运行环境（探测本机运行时版本）、读取 Git 和工作区上下文并持久化快照。独立重放同样四条只读 Git 命令，三轮合计约 0.73s、0.39s、0.39s；这说明本机子进程与文件系统准备确实消耗时间，但不是逐函数剖析，不能据此把全部 3.3s 都归给 Git。本轮没有通过跳过权限、工具、上下文或持久化来缩短这些步骤。

当前修复消除了整页等待聊天代码加载的路径；**没有达到所有电脑、所有交互 10ms 内完成的要求**。CPU 减速下仍会变慢；正文的首 token 时间还包含供应商选择工具及工具后的再次请求。

本轮构建和机器日志、截图放在忽略目录 `target/chat-ui-audit-20261118/`。未将用户数据库、密钥或原始模型日志加入版本控制。

验收对应的源码和可执行文件 SHA256 保存在 `validated-inputs.json`。最终复核时其中列出的生产源码均保持一致，但桌面可执行文件的哈希已变化；本报告的原生验收结论仅对应已记录哈希的当时构建，不扩展为之后可执行文件的验证结论。

隔离验收窗口已关闭，重新编译的程序随后曾使用原数据根重新打开，并移除测试时的 CDP 启动参数；测试产生的模型选择变更已恢复。最终检查时未见仍在运行的桌面进程。Git 工作区原本已有大量未提交的多协议注册表和 UI 重构；本轮依赖其中尚未进入 HEAD 的模型选择接口，独立补丁检查未通过，因此没有创建混合提交或更改已有暂存内容。

## 提交前复核（2026-11-18）

用户随后授权整理可提交改动，模型注册表依赖已提交为 `ec0c9708`。提交前发现思考候选显示 `off` 时也填入 `/thinking off`，但共享后端契约接受 `none`；新增测试直接执行 Composer 的真实候选生成副作用，先复现失败，再将生成的关闭命令修正为 `/thinking none`，界面仍显示 `off`。

`pnpm build` 的 544 项测试、TypeScript 和 Vite 构建通过；Composer 定向回归 31 项通过，修正后的源码另经 `tsc --noEmit` 和 Vite 生产构建通过。模型与持久化 644 项通过、4 项忽略，注册表集成 51 项通过，SDK 204 项通过；已修改 Rust 文件格式检查通过。全仓格式检查仍仅报告未修改的 `deepagent-core/src/id.rs`，全仓测试的历史失败没有在此次提交整理中修复，也没有宣称完整门禁通过。

本次复核没有重新调用真实供应商或启动原生桌面，原生验收范围仍以上文及 Start→Chat 剖析报告记录的构建为准。当前 App、设置页、模型页、Slash 路由和候选滚动源码与上轮原生验收记录一致；Composer 仅额外修正了上述关闭命令映射。命令及输出保留在忽略目录 `target/commit-audit-20261118/`。
