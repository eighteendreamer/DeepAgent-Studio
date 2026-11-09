# DeepAgent Studio 内置 Linux 升级实施蓝图

> 版本：2；核查日期：2026-11-09。本文把原方案展开为源码改造与交付单元，尚未实现产品 VM 后端。
> 发布阻塞：无 WSL 的目标 Windows 11 电脑曾启动失败，根因未闭环；生产双向通道、只读基盘、非 root 和 3 GB 硬上限均未验收。不得据此宣布可正式发布。

先看[架构图册](diagrams/linux-vm/index.html)，再按本文 UP-00～UP-12 执行。[原主方案](2026-11-09_linux-vm-architecture-upgrade-report.md)保留完整策略背景，[接口附录](linux-vm-contracts-and-storage-policy-v1.md)定义拟议 DTO/RPC，[验收索引](linux-vm-evidence-and-acceptance.md)保存 E01～E22 和 V01～V17 的含义。本文新增类型、文件、表、接口均为**本地设计**，不存在的文件明确标为“新增”。

## 1. 这次架构升级到底改变什么

交付形态是：**原 Agent 控制内核留在宿主 Rust；增加一个 Linux 执行目标、一个独立 VMM worker 和一个 Linux 执行服务；把执行目标、工作副本、资源预算和生命周期接入原运行准备与审计链。** Tauri 保持窗口与桥接职责，CLI/SDK 也使用相同 AppCore 能力。

| 层次 | 继续复用 | 具体升级 | 不迁入 Linux 的内容 |
|---|---|---|---|
| 展示与入口 | React、Tauri、CLI、Harness | 目标选择、准备进度、空间与诊断；DTO 同步 | 窗口、用户审批、系统桌面操作 |
| 运行控制 | ChatService、RunCoordinator、AgentKernel、ToolRegistry | 运行准备绑定执行计划；同一取消与审批作用域 | 模型连接、密钥、唯一终态与历史 |
| 工具执行 | CommandExecutor、SandboxBackend、严格 code profile | 新增 LinuxVm 后端；目标感知的运行时绑定 | 原宿主 Direct/Sandboxie/WindowsSandbox |
| 资源控制 | RuntimeService、UserPaths、安装归属、原数据库 | 镜像激活、原子预留、scope 卷租约、清理调度 | 用户原项目和已交付产物的所有权 |
| 隔离执行 | Demo 的可行性证据 | 固定 ABI worker、版本化通道、非 root guest-agent | 第二套 Agent、审批中心或事件存储 |

以下不是同一个选项，必须分开建模：

1. `executionTarget`：在哪个操作系统执行，host / linux_vm。
2. `sandboxBackend`：使用哪种隔离执行器；Linux 目标只能绑定 LinuxVm。
3. `runtimePreference`：在已经选定的目标内选择工具来源。现有 managed_only 仍指宿主管理工具，不能修改旧语义。
4. `permissionProfile`：是否获准执行、读写哪些文件、访问哪些网络；探测到工具并不等于获准使用。

示例：用户只要求运行 Python，宿主有合适 Python 且已授权则选 host；宿主缺失时整项任务选 Linux，并使用 Linux 的 Python、Git 和项目依赖。不能每条命令单独切换目标。若任务中途发现缺少先前未声明的能力，停下并重新准备/批准任务；不在同一个已批准执行上下文里切换操作系统。

全系统能力归属必须逐项明确，而不是把“Rust 后端”理解为全部搬进 VM：

| 现有功能面 | 升级后所在位置 | 本轮方案要求的改变 |
|---|---|---|
| 模型适配、上下文、记忆、知识、规划与技能 | 原 deepagent-models/context/memory/knowledge/planner/skills 与 AppCore 宿主服务 | 不迁移数据和模型调用；工具执行上下文增加目标事实 |
| 会话、输入接管、审批、取消、运行事件 | 原 Kernel、RunCoordinator、RunStore、InputLeaseRegistry | 只投影新目标与执行证据，权威保持一份 |
| Bash、构建、测试、Git CLI、工作流 code | 原工具链分派到 host 或 Linux | UP-05/07；命令与严格 code profile 分开验收 |
| 普通文件工具与 Linux 文件变更 | 宿主权威文件；Linux 受管工作副本 | UP-06；原文件门禁与共享提交边界，冲突不覆盖 |
| HTTP 工具、模型 API、HTTP MCP | 原宿主连接及权限 | 保持原入口；guest 出口另外接 UP-08，不继承所有宿主网络权限 |
| stdio MCP、插件子进程、项目 hook | 按能力声明分派；业务 registry 留宿主 | UP-09；原生桥接和宿主生命周期 hook 不随项目脚本迁移 |
| 本机/SSH 终端与新增 VM 终端 | 原宿主/SSH 后端继续存在；新增 VM backend | UP-09；复用终端租约、接管和 cursor |
| Office/PDF/音视频、真实浏览器/桌面、SSH/移动设备 | 原宿主、远程或设备专用服务 | 不承诺 VM 自动覆盖；Linux 可移植子任务逐项声明、逐项验收 |
| Linux 开发服务的浏览器预览 | guest 服务 + 宿主受控预览代理 + 原预览 UI | UP-08/09；显式端口租约、来源限制和退出回收，不暴露整张 guest 网络 |

因此“尽可能通过全系统功能”的验收方式是保留原能力的回归，加上每个迁移入口的目标测试矩阵；不是把 Demo 的通用命令测试当成所有业务模块已经验证。

## 2. 七张图分别回答什么

颜色和前缀同时表达改造类型：**R 灰色＝保留；M 蓝色＝扩展现有边界；N 绿色＝新增；G 琥珀色虚线＝发布前验证门槛。** 时序图描述未来契约，不表示 API 已实现。实线是调用/数据流，虚线是控制依赖；时序图虚线箭头表示回复。

| 图 | 解决的问题 | 图源 |
|---|---|---|
| [01 部署与信任边界](diagrams/linux-vm/architecture.svg) | 哪些进程在宿主，哪些在 VM，权限与持久化由谁决定 | [Mermaid](diagrams/linux-vm/architecture.mmd) |
| [02 源码组件改造](diagrams/linux-vm/components.svg) | 当前 RuntimeBroker、装配器、code 节点和终端分别在哪里改 | [Mermaid](diagrams/linux-vm/components.mmd) |
| [03 命令完整时序](diagrams/linux-vm/execution.svg) | 审批、空间、派发、输出、取消和终态如何串起来 | [Mermaid](diagrams/linux-vm/execution.mmd) |
| [04 文件导入与提交](diagrams/linux-vm/workspace.svg) | Linux 怎样改用户文件，发生并发编辑时怎么办 | [Mermaid](diagrams/linux-vm/workspace.mmd) |
| [05 存储与所有权](diagrams/linux-vm/storage.svg) | 文件放哪里，3 GB 算哪些内容，谁能删除 | [Mermaid](diagrams/linux-vm/storage.mmd) |
| [06 清理与恢复](diagrams/linux-vm/maintenance.svg) | 定时/压力/手动清理怎样共用一条可靠链 | [Mermaid](diagrams/linux-vm/maintenance.mmd) |
| [07 启动与运行状态](diagrams/linux-vm/lifecycle.svg) | 初始化不阻塞窗口；不可用、重试、崩溃怎样处理 | [Mermaid](diagrams/linux-vm/lifecycle.mmd) |

图 01 中的 GuestNetworkBroker 箭头表示经过 worker 的逻辑流，不能理解为 guest 可绕过 worker 直接调用 AppCore。图中各接口的传输与权限约束见下一节。

## 3. 九个交互边界：谁调用谁，传什么

| ID | 生产方 → 消费方 | 契约与数据 | 权威、失败和兼容要求 |
|---|---|---|---|
| I01 | React → Tauri → ChatService | 原 start_chat_v2/审批/事件；管理页新增环境和存储 DTO | UI 不直接拿 worker/RPC 句柄；桥接参数在 Rust 重验 |
| I02 | CLI/SDK → Harness adapter → AppCore | 原 thread/turn/approval/config/sandbox 能力 | 和 I01 共享准备结果；CLI 入口不得绕开 Linux 权限 |
| I03 | VmService → vm-worker | 私有启动描述、已验证 manifest、磁盘/IPC 句柄、boot generation | 不接受模型给出的 DLL 或磁盘路径；worker 退出由宿主观察 |
| I04 | worker ↔ guest-agent | guest/hello、exec/start、signal、output、exited；独立帧与数据流 | 控制与 stdout 分离；版本/长度/ID/generation/限流检查；P0 先验证双向设备通道 |
| I05 | WorkspaceProjection → 原文件边界 | source handle、projectionId、相对路径、baseline、候选内容与变更集 | guest 路径只是输入；宿主决定读写授权、冲突、发布与实际记录 |
| I06 | guest → worker → GuestNetworkBroker | 已批准网络范围 ID、目的地、流 ID；后续才支持代理流 | 默认 guest 无直接网络；不信任 guest 自报批准；目的地址/重定向由宿主验证 |
| I07 | Kernel/适配层 → 原 stores → UI/SDK | 原 run_events + 执行上下文、工具进度、原文引用、产物引用 | run sequence 和唯一终态由原链生成；exec 输出序号不是 run sequence |
| I08 | 下载/导入/日志/建卷 → ManagedStorage | reserve/commit/release、resourceId、owner、upperBound、lease | 同一事务化控制，先承诺再写；超限拒绝，取消不提前释放存活资源 |
| I09 | Rust 调度/管理 UI → 维护服务 | reason、policy revision；preview/apply 复用同一计划器 | 定时、压力、手动一条链；跨进程独占、资源 claim、删除意图与恢复 |

I03 可以先用私有 stdio/继承管道实现监督协议，但不把 stdout 当 VM 控制通道；具体 OS 句柄继承规则要验收。I04 首选独立 virtio-console 控制端口，现 Demo 只证明过默认输出路径，未证明 multiport 双向生产能力。该门槛失败时先重新选择传输方案，不先写死平台 ABI 或公开无认证 TCP 端口。

## 4. 当前源码暴露的真实改造缺口

下列行号只是核查定位，实施时按符号重新搜索；不能把行号当稳定 API。它们是原证据索引的补充，均为 2026-11-09 的源码观察。

| Evidence | 当前位置/符号 | Finding | 对应 Path / 升级项 |
|---|---|---|---|
| E31 | [run_assembler.rs](../crates/deepagent-app-core/src/run_assembler.rs)，base_registry_request（约 1880） | 当前给已有 executor 包 RuntimeCommandExecutor，会注入宿主 SDK 环境；仅加 LinuxVm 分支会污染 guest 环境 | I01/02 → 目标绑定 → I04；UP-01/05 |
| E32 | [tool_runtime.rs](../crates/deepagent-app-core/src/tool_runtime.rs)，RuntimeCommandExecutor::environment | build_process_environment 使用宿主项目与 RuntimeBroker；不是跨 OS 环境生成器 | 同 E31；UP-05 |
| E33 | [workflow_code.rs](../crates/deepagent-app-core/src/workflow_code.rs)，WorkflowCodeGateway::execute | managed_requirement + broker.resolve 在宿主解析 SDK；SandboxCodeRequest.runtime 也是宿主 RuntimeResolution | strict code → 目标 SDK binding → 原 backend；UP-07 |
| E34 | [file_mutation.rs](../crates/deepagent-builtins/src/file_mutation.rs)，apply / Mutation | apply 是 pub(crate)，Mutation 面向文本；AppCore 不能直接把任意二进制导出“调用原函数” | I05 → 共享提交基础设施 → 用户文件；UP-06 |
| E35 | [fs_guard.rs](../crates/deepagent-builtins/src/fs_guard.rs)、[fs_commit.rs](../crates/deepagent-persistence/src/fs_commit.rs) | WorkspaceRoot、文件身份和提交锁已有基础；内部门禁不能复制成 VM 专用第二份 | 同 E34；UP-06 |
| E36 | [run_environment.rs](../crates/deepagent-app-core/src/run_environment.rs)、[execution_config.rs](../crates/deepagent-app-core/src/run_assembler/execution_config.rs) | 目标后端已按 run 解析；新目标应进入同一配置快照，不能只存在 React 状态 | I01/02 → 原 prepare/admission；UP-01/11 |
| E37 | [runtime_log_store.rs](../crates/deepagent-persistence/src/runtime_log_store.rs)、[managed_files.rs](../crates/deepagent-app-core/src/managed_files.rs) | 原日志有独立写入者，inventory 是观察；二者都不能替代所有写者共享的空间预留 | I08/09 → 原存储生命周期；UP-03/10 |
| E38 | [Tauri lib.rs](../apps/desktop/src-tauri/src/lib.rs)、[CLI main.rs](../apps/cli/src/main.rs)、[app_server.rs](../apps/cli/src/app_server.rs) | Desktop 和 CLI 都有装配点；只在 Tauri 注册后端会遗漏无头入口 | I01 与 I02 收敛；UP-05/11 |
| E39 | [runtime_service.rs](../crates/deepagent-app-core/src/runtime_service.rs)，RuntimeKind / RuntimeArtifact / RuntimeBroker | 当前三种 SDK 与 Platform 为宿主执行设计；Git 不在 RuntimeKind 中；guest 镜像不能伪装为 Windows SDK | 资源 registry → 镜像激活 → guest 工具 manifest；UP-02 |

可复核命令（PowerShell，在仓库根运行；只读）：

```powershell
rg -n 'base_registry_request|RuntimeCommandExecutor|local_executor_for_run' crates/deepagent-app-core/src/run_assembler.rs
rg -n 'managed_requirement|broker.resolve|SandboxCodeRequest' crates/deepagent-app-core/src/workflow_code.rs
rg -n 'pub\(crate\).*apply|enum Mutation|lock_path_operation' crates/deepagent-builtins/src/file_mutation.rs
rg -n 'SandboxBackendRegistry|sandbox_backend_registry' apps/desktop/src-tauri/src/lib.rs apps/cli/src
```

## 5. 可以直接排期的改造单元

每项“完成”同时要求源码契约和对应验收通过。文中的新路径是实现目标，不是当前文件。原 P0～P5 是交付门槛，UP 是可以分批审查的改动；二者不混用。

### UP-00：先解决平台与执行通道基线（P0）

- 对象：独立 Demo 的 platform/preparation/engine/runner 及可靠性记录，不先接业务后端。
- 输出：锁定引擎来源 commit、二进制 hash、准确头文件/固件/许可证；验证双向通道、只读 root、固定 raw 数据卷、非 root、worker 回收；目标无 WSL 机器导出原始失败并复测。
- 门槛：V01/V02 和 V09 的前置能力；明确区分“系统条件不满足”“库加载失败”“Linux 启动失败”。缺 WHP/BIOS 虚拟化时展示必要操作；不自动改系统设置。
- 依赖/回退：本项未通过，后续只能开发接口与受控实验，不能把 Linux 设为可用或替换旧版执行路径。

### UP-01：将执行计划纳入原运行准备（P2 的准备工作）

- 修改：[run_environment.rs](../crates/deepagent-app-core/src/run_environment.rs)、[execution_config.rs](../crates/deepagent-app-core/src/run_assembler/execution_config.rs)、[prepared_chat.rs](../crates/deepagent-app-core/src/run_assembler/prepared_chat.rs)、[run_config.rs](../crates/deepagent-app-core/src/run_config.rs)、[settings.rs](../crates/deepagent-app-core/src/settings.rs)；新增 `execution_environment.rs`。
- 行为：解析 task requirements、显式目标、权限、运行时版本；只读 status/resolve 与 prepare 使用同一个解析器。prepare 冻结 planId、目标、镜像版本、scope、工具 binding 和策略修订；变化后必须重新准备，不能批准旧快照再执行新目标。
- 契约：原 PreparedChatRun 携带计划；既有配置缺省保持旧行为。仅识别字符串 linux_vm 不等于后端可用。
- 验收：V04/V05/V06；本地完整/缺失/版本冲突/部分匹配、显式目标不可用、配置并发变化、无执行权限均有真实准备测试。依赖 UP-00 的平台状态接口；关闭 VM 策略只影响未来 run。

### UP-02：把镜像作为资源安装，而非宿主 SDK（P1）

- 修改：[runtime_service.rs](../crates/deepagent-app-core/src/runtime_service.rs)、[runtime_payload.rs](../crates/deepagent-app-core/src/runtime_payload.rs)、[install_ledger.rs](../crates/deepagent-app-core/src/install_ledger.rs)、[data_root.rs](../crates/deepagent-app-core/src/data_root.rs)；新增镜像激活模块，挂在同一 RuntimeService 生命周期。
- 行为：artifact 显式区分 host SDK 与 guest image，分别表达 host 可加载架构和 guest OS/arch；保留旧 artifact 默认 host。随包 seed 离线激活，更新资源下载到有界 staging，验证完整性后原子更新 active 指针。
- 契约：继续复用 runtime_list/status/install/cancel/uninstall/progress；guest 工具来自镜像 manifest 与启动探测，不把 `/usr/bin/python3` 交给宿主 resolve_command。Git 纳入 guest manifest 不要求顺手扩充宿主 SDK 支持。
- 验收：V07/V08/V16/V17；取消/损坏包/架构不匹配/升级双版本峰值/自定义资源根；active 切换前失败保留旧版。依赖 UP-03 的预留控制。

### UP-03：先把写入变得有界，再谈自动清理（P1）

- 修改：[managed_files.rs](../crates/deepagent-app-core/src/managed_files.rs)、[managed_file_store.rs](../crates/deepagent-persistence/src/managed_file_store.rs)、[migrations.rs](../crates/deepagent-persistence/src/migrations.rs)；新增 `managed_storage.rs` 与同库资源控制访问模块。
- 行为：所有建卷、下载、解压、导入、导出、脚本及 VM 原文写入都走 reserve→write→commit/release。固定 raw 卷按上界计费，下载/输出按有界块滚动预留；失败后核对实际资源才释放承诺。
- 数据：见 §6；不新建 JSON 配额账本，不用 inventory 扫描结果充当锁；安装归属继续由 InstallLedger 管，结果引用继续归 ArtifactStore。
- 验收：V08/V09/V13/V16；多进程恰好越限、声明长度造假、宿主磁盘不足、inode 满、崩溃遗留、迁移旧资源。旧数据首次盘点标记来源，身份不明者保留并报差异，不自动删除。

### UP-04：监督器、worker 与 guest-agent（P0/P2）

- 新增：`deepagent-app-core::vm_service`；独立 `apps/vm-worker/`、Linux 目标 `apps/guest-agent/`；仅在确需双端序列化复用时增加小型内部协议 crate。
- 行为：VmService 持有 worker 进程和租约；worker 只接收已验证引擎与资源描述。guest 控制帧、stdout/stderr、文件流分开且有界；每次启动有新 bootId，过期消息不被接受。
- 调度：默认全应用最多一台运行 VM。不同 scope 排队；切换项目/权限/镜像/网络或 strict-code profile 时排空进程并销毁重建，不能只换 cwd。排队也绑定原取消与 deadline，不能无限等待。
- 验收：V02/V05/V09/V17；真实进程创建/超时/EOF/孤儿/崩溃/大流量/版本不符/过期 generation；清空宿主凭据环境。依赖 UP-00、UP-03；不支持的通道或平台明确 unavailable。

### UP-05：接到现有命令链，消除宿主环境污染（P2）

- 修改：[sandbox_backend.rs](../crates/deepagent-app-core/src/sandbox_backend.rs)、[run_assembler.rs](../crates/deepagent-app-core/src/run_assembler.rs)、[tool_runtime.rs](../crates/deepagent-app-core/src/tool_runtime.rs)、[chat_service.rs](../crates/deepagent-app-core/src/chat_service.rs)；新增 `sandbox_backend/linux_vm.rs`。Desktop 和 CLI 的 composition root 注册同一后端实例/服务配置。
- 行为：新增 LinuxVm 枚举与序列化值，保持 CommandExecutor 和原 BashTool schema；target=host 才套宿主 RuntimeCommandExecutor，target=linux_vm 由 guest binding 提供环境。cwd 从授权 projection 解析为 guest handle，而非字符串替换盘符。
- 契约：原 cancel/deadline 必须传到真实子进程；保持命令退出码和输出截断事实，区分 worker 退出。准备/启动/执行失败禁止静默切 Direct。先禁网发布实验能力，UP-08 验收后才支持授权网络。
- 验收：V04/V05/V06；同一 fixture 经 CLI/Desktop/Harness 得到一致结果；退出 23、Unicode、超时、取消、断线 replay、不重复终态；host SDK PATH 不进入 guest。依赖 UP-01/03/04/06。

### UP-06：将“复制出来”升级为有冲突保护的文件提交（P2）

- 修改：[fs_guard.rs](../crates/deepagent-builtins/src/fs_guard.rs)、[file_mutation.rs](../crates/deepagent-builtins/src/file_mutation.rs)、[file_cache.rs](../crates/deepagent-builtins/src/file_cache.rs)、[fs_commit.rs](../crates/deepagent-persistence/src/fs_commit.rs)；新增 AppCore `workspace_projection.rs`。
- 行为：从原门禁提取最小、受测、可跨 crate 使用的 bytes-safe 提交契约，现有文本 write/edit 与 VM 导出共用身份检查、观察版本、提交锁、staging 和缓存失效；不得复制成第二个 VM 写入器，也不能简单把 pub(crate) 改 pub 后跳过原约束。
- 能力：先支持二进制安全的新增/修改文件；删除、重命名和符号链接按独立操作授权及测试后开放。导入时记录基线，导出时复核宿主当前内容，冲突保留提案不覆盖。多文件逐项提交，不承诺项目级原子事务；部分提交要有可恢复记录。
- 验收：V10/V13；并发用户编辑、read-before-edit 不回退、恶意相对路径/ADS/保留名/大小写冲突、链接替换、取消、二进制输出、部分提交中断。依赖 UP-03；默认不提供直接可写宿主挂载。

### UP-07：严格工作流代码执行单独绑定（P3）

- 修改：[workflow_code.rs](../crates/deepagent-app-core/src/workflow_code.rs)、SandboxCodeRequest 和相应 runtime 类型；新增 `GuestRuntimeBinding`，由已准备计划创建。
- 行为：Host(RuntimeResolution) / Guest(binding) 显式区分。保留一次性 code handle、原 Kernel 授权与消费语义；guest strict code 没有默认 workspace/network grant，不能和项目 Bash 共用可写状态来宣称严格隔离。
- 验收：V07/V09；Python/JS 代码句柄只消费一次，取消发生于 SDK 解析前后、损坏绑定、版本不符、scope 污染和不确定退出不自动重放。依赖 UP-01/04/05。

### UP-08：网络是新的策略执行点（P3）

- 修改：[http_runtime.rs](../crates/deepagent-app-core/src/http_runtime.rs)中可复用的地址校验；新增 AppCore `guest_network` 适配；VM 设备配置保持 guest 无直接出口。
- 行为：宿主 broker 校验批准范围、最终连接 IP、DNS/重定向和私网规则；仅普通 HTTP fetch 不足以支撑 pip/npm/Git/Maven，代理流与 TLS CONNECT、SSH Git、代理认证分别声明支持。支持不了就返回 NetworkDenied/不支持，不临时打开整张网卡。
- 凭据：模型 API、宿主 SSH agent、用户全局代理认证不自动传入 guest。私有依赖需要 scoped credential，限制目的地、时间与日志脱敏；不把凭据写入脚本或日志。
- 验收：V11；四套包管理器真实下载/证书，deny 模式、重绑定、重定向、IPv6、loopback/私网、认证泄漏和出口绕过。依赖 UP-04/05；未通过前仅声明离线能力。

### UP-09：交互与集成功能逐入口迁移（P3）

- 终端：修改 [terminal_service.rs](../crates/deepagent-app-core/src/terminal_service.rs)、[deepagent-terminal](../crates/deepagent-terminal/src/lib.rs)，新增 VM TerminalSessionBackend，保留 open/read/cursor/write/resize/signal/renew/takeover 和 lease epoch。原终端 UI 复用，不能绕开租约另开 WebSocket。
- MCP：修改 [mcp_runtime.rs](../crates/deepagent-app-core/src/mcp_runtime.rs)、既有 MCP 进程 transport；VM stdio server 启停关联 scope/lease。请求相关性与 catalog generation 保持原义，崩溃后不重放有副作用工具。HTTP MCP 留宿主。
- 插件/hook：审查 [plugin_runtime.rs](../crates/deepagent-app-core/src/plugin_runtime.rs)、[hook_runtime.rs](../crates/deepagent-app-core/src/hook_runtime.rs)、payload 的全部 spawn 调用；逐能力声明执行域。需要本机浏览器、Office、设备或原生库的插件继续走原宿主服务。
- 预览端口：新增宿主预览代理适配，登记 exec/scope/guest port 与显式租约，仅对授权 UI 开放本机 loopback 入口；验证短期访问凭据、Origin、WebSocket 与退出回收。guest 服务后台运行必须有续租，不能靠命令结束后遗留的进程维持。它是拟议能力，I04 通道和代理传输验证前不声明支持。
- 验收：V07/V11/V14；终端输入抢占/旧 lease/replay/断线，MCP 断连与副作用不重放，插件只获声明权限，真实开发服务器的 HTTP/WebSocket 预览与回收。依赖 UP-04/05/08；各支持位独立，不能一个通过就把其余标为支持。

### UP-10：定时清理、历史过期与恢复（P4）

- 修改：[service.rs](../crates/deepagent-app-core/src/service.rs)、[host_lock.rs](../crates/deepagent-app-core/src/host_lock.rs)、原 ArtifactStore 删除协议、RuntimeLogStore 写入者和 settings；调度挂在 AppCore 服务生命周期，不在 React setInterval。
- 行为：默认启动后 30 秒、每 15 分钟、任务后 60 秒与压力触发共用 plan/claim/revalidate/delete/commit。维护锁失败跳过；待删 claim 原子阻止新租约；删除前再次核对真实进程、文件身份与引用，崩溃后幂等恢复。
- 结果：成功原文 3 天、失败/取消原文 7 天、临时脚本 30 分钟；其他 TTL 沿用附录。过期原文保留摘要与 tombstone；会话 run_events 的 replay 生命周期保持原策略。guest 缓存清理只报告 guest 块，不能假报宿主 raw 文件缩小。
- 验收：V12/V13/V15；活动/pinned/replay 引用不删，竞态新租约、路径替换、重启与删除失败、配置更新、所有候选被保护时拒绝新写入。依赖 UP-03/05 与原产物删除协议；最低限度保护随 P1/P2 即上线，P4 只补完整治理。

### UP-11：前端、Tauri、CLI 和 Harness 同步交付（P2/P4）

- 修改：[api.ts](../apps/desktop/src/api.ts)、[types.ts](../apps/desktop/src/types.ts)、[ConfigSettings.tsx](../apps/desktop/src/components/settings/ConfigSettings.tsx)、Tauri lib.rs/capabilities、[requests.rs](../crates/deepagent-harness-protocol/src/requests.rs)、CLI app_server/args 与 [TypeScript SDK](../packages/sdk/package.json)。
- 行为：聊天入口保持原链。环境页调用 status/resolve/prepare，空间页调用 status/preview/apply；修改的是统一 SettingsService 的策略片段，带 expectedRevision，不覆盖并发模型/密钥设置。DTO 与 Harness DTO 分开，映射同一 AppCore 状态。
- 展示：任务目标及原因、工具版本、初始化阶段、不可用错误、池占用/guest 空闲、可回收与受保护字节、日志过期及文件冲突；UI 就绪不等待解压/校验/启动。可信主窗口才有管理命令，远程预览不得继承。
- 验收：V05/V06/V12/V14；旧客户端/旧配置、unknown backend、设置并发、失败时导出诊断、invoke/事件/replay 与真实安装闭环。依赖 UP-01/05/10；原终端/设置组件复用 shadcn 体系。

### UP-12：打包、升级与回退（P5）

- 修改：[tauri.conf.json](../apps/desktop/src-tauri/tauri.conf.json)、[installer/hooks.nsh](../apps/desktop/src-tauri/installer/hooks.nsh)、[release-desktop.yml](../.github/workflows/release-desktop.yml)和 worker/guest 的目标构建配置。
- 行为：按 host OS/arch 携带一套对应资源；Linux 工具只在镜像内，宿主可选 SDK 不删。安装器拥有 bundled 文件，激活派生资源由用户数据根管理。卸载精确识别主 CLI/worker，不能依赖 deepagent*.exe 通配符误调用 worker。
- 升级：在同一 3 GB 池预算内验证新旧版本与 staging 峰值；放不下就暂停升级、要求用户选择回收可回滚版本或延后，不能临时越限。rollback 指向仍完整且兼容的旧资源，只影响未来任务。
- 验收：V01/V03/V16/V17；release 真安装器、离线首用、低磁盘、普通用户路径、升级失败/断电、卸载保留用户项目、旧执行链回归。Windows x64 先验收，其他平台逐个开放；包小于 1 GB 必须测完整产品产物。

## 6. 最少新增哪些控制数据

下表是**同一主数据库内拟议控制关系**，不是第二份 run/event store。列名和具体 SQL 在实现时经 migration 测试定版；这里固定职责与一致性要求。既有数据无损兼容，新增字段缺省不得改变原宿主行为。

| 控制关系 | 最小事实 | 写入权威与不变量 |
|---|---|---|
| 执行计划引用 | runId、planId、target、snapshot hash、image digest、scope、policy revision | 原运行准备/RunStore 上下文；批准后不可原地改目标 |
| 受管资源控制 | resourceId、kind、owner/ref、路径身份、已计量字节、generation、state | ManagedStorage 控制生命周期；既有安装和产物仍以原表/清单为权威，不复制其元数据另作主记录 |
| 预算预留 | reservationId、operation/run owner、upperBound、已提交字节、state | 与资源状态事务协调；不能先释放承诺后让写入继续 |
| 资源租约 | resourceId、scope、owner process/bootId、generation、lease state | 标记活跃性；时间过期不等于真实进程已死；删除 claim 与新 lease 原子互斥 |
| 维护意图 | operationId、reason、policy revision、candidate generation、state、结果引用 | 恢复与幂等控制；审计摘要写回原 RuntimeLogStore，不造清理日志库 |
| 文件 projection 提交记录 | projectionId、基线引用、候选 hash、已批准变更、逐项提交状态 | 宿主文件协调边界；既有 ArtifactStore 保管候选/结果，记录部分提交以恢复 |

关联标识保持清晰：`runId` 是整个 Agent run，`actionId` 是原工具/权限动作，`execId` 是一次命令，`bootId` 是一次 VM 启动，`scopeId` 是数据与信任范围。不能用其中一个冒充其他标识；文件提案来自某个 exec 也不自动获得写授权。

## 7. 3 GB、目录和清理如何同时成立

图 05 的 350 + 60 + 1,000 + 1,000 + 100 + 300 + 190 = **3,000 MB** 是生产打样预算，不是 Demo 现状。Demo 仍是 2 GiB 基盘和 qcow2 写层；不能把新预算套上去就宣布合规。完整产品安装包 <1 GB 和所有 Linux 受管资源 ≤3 GB 是两个不同指标。

同一物理文件只计一次。首次基盘解压使用基盘类别的候选预留（最多 1,000 MB），校验后同卷 rename 发布；不能先占一个 1 GB 激活基盘，又把另一份 1 GB 解压副本藏进 300 MB 暂存类别。下载包、旧基盘和新候选的共存峰值独立计算；若原数据卷受保护且无法腾出升级空间，明确延后升级。分类预算可按已批准方案重分，但全局硬上限不变。

目录以 UserPaths 和 RuntimeService 的实际活动根为准，禁止硬编码 `%LOCALAPPDATA%` 作为正式产品统一根。Demo 的数据目录只用于实验。新增文件的规则是：

| 内容 | 放置/所有权 | 清理与计量 |
|---|---|---|
| 安装包附带引擎、worker、压缩镜像 | 安装资源 vm-engine / vm-seed；安装器所有 | 被安装副本计入 Linux 池；普通缓存清理不擅自删安装资源 |
| 激活基盘 | RuntimeService 活动根/linux-vm/images/digest | 同时在用/回滚的版本都计量；无引用且过保护期后回收 |
| scope 卷 | UserPaths.files/vm/volumes/id/data.ext4 | 所有卷上界合计≤1,000 MB；guest 清缓存不减小宿主 raw 长度 |
| 下载/解压/脚本/导出暂存 | files/cache/vm 与 files/vm/exports | 总临时预算 300 MB；失败与取消仍保留归属后按策略回收 |
| VM 大原文日志 | 原 tool-results，ArtifactStore 引用 | 总预算 100 MB；单次上限默认 8 MiB；过期保留执行摘要 |
| 操作与授权摘要 | 原 deepagent.db / runtime-logs.db | 通用元数据库按原策略治理，不夹带无界 VM 原文；本身仍要做容量和写失败处理 |
| 用户原项目和已发布文件 | 用户选择目录/原产物生命周期 | 不归 VM 缓存清理；目标同目录 staging 未发布前仍计入暂存预算 |

guest 数据使用率达到 70%/85%/95%，分别执行空闲缓存淘汰、暂停新写入准备、拒绝新写入型任务并按策略停止持续写入。宿主池达到 85%/92%触发有界维护/停止新分配。**硬上限依靠每次写前预留和设备上界，定时任务只负责复用空间。** 所有资源都受保护时明确 QuotaExceeded，不删除用户数据满足指标。

## 8. 交付顺序与逐批可审查结果

| 批次 | 包含改造 | 必须可展示的结果 | 不满足时停在哪里 |
|---|---|---|---|
| A · 基线与契约 | UP-00、UP-01 的类型/只读解析 | 目标机原始诊断；双向协议与卷证据；host 选择回归 | 不注册可执行 VM 后端，不改默认目标 |
| B · 资源与生命周期 | UP-03 → UP-02、UP-04 | 离线激活、硬预算、可靠停止/恢复和阶段进度 | 不接可写用户文件，不做网络能力声明 |
| C · 单次任务闭环 | UP-06、UP-05、UP-11 最小展示 | 原审批 → VM 命令 → 候选回写/冲突 → 原审计；CLI/Desktop 同结果 | 仅受控实验；不宣布全产品功能通过 |
| D · 开发与交互 | UP-07、UP-08、UP-09，各自独立验收 | strict code、包管理联网、PTY、MCP/插件的支持矩阵 | 未通过项保持不支持，不扩大能力位 |
| E · 完整治理 | UP-10、UP-11 完整设置与历史 | 定时/压力/手动共用清理；原文过期后仍可追溯 | 保持试验开关，配额机制不可关闭 |
| F · 平台发布 | UP-12 | 新电脑真安装、离线首用、完整包体积、升级与回退 | 不向未验收平台发可用声明 |

UP-03 与 UP-04 的接口可分别实现，但生产启用依赖同一资源协议。不要把“并行写几个模块”理解为可以跳过批次门槛。每批提交只覆盖对应模块与所有必要消费者，旧路径保持可回归；不一次性改整个内核。

## 9. 必须固定的失败语义

| 场景 | 用户看到的结果 | 系统必须保留的事实 |
|---|---|---|
| VM/组件不可用 | 阶段、原错误码、必要操作，允许导出诊断 | 没有派发命令；依赖用例被阻塞；未转宿主 |
| 命令退出 23 | 命令执行结束，exitCode=23 | worker 退出码与命令退出码分别记录；run 由 Kernel 决定 |
| 取消/超时 ACK 后进程仍存活 | 正在终止或恢复中 | 实际进程与租约仍在；不能先清盘 |
| 文件冲突/部分发布 | 精确逐项结果；不覆盖新内容 | 已提交项、候选和恢复引用；不虚报整批原子成功 |
| 输出/原文预算耗尽 | 截断或原文过期标记 | 控制通道继续有界 drain；审计摘要与真实退出保留 |
| 清理失败/路径替换 | 跳过原因、保护项、未回收字节 | 只按实际删除更新账目；旧身份不授权删新文件 |
| guest 声称操作成功但宿主未提交 | 候选结果/待提交，不显示已改宿主文件 | 可信执行记录是宿主已授权并实际观察的边界操作 |

## 10. 实施验证与本次文档验证要分开

实施每批至少运行适用 crate 的格式/测试；共享跨 crate 行为运行 workspace 测试；Rust/TypeScript 或 Tauri 契约变化运行桌面构建，UI 运行真实前端验证。新增持久化/终态/replay 必须有迁移、重启和重连测试；文件与删除边界必须有真实失败回归，不能只断言 DTO 字段。

当前仓库可用验证入口如下；**本次仅改文档和图，不以这些未来验证命令冒充已执行**：

```powershell
cargo fmt --all -- --check
cargo test --workspace --offline
cargo clippy --all-targets --all-features -- -D warnings
pnpm --dir apps/desktop build
```

本次实际完成情况和图册复现方法见[图册说明](diagrams/linux-vm/README.md)。所有 SVG 来自相邻 Mermaid 源码；HTML 图册离线工作，提供缩放、展开和源文件链接。保留原根目录 system-architecture.svg 作为既有全系统资料，不把本次未来 VM 方案伪装成当前系统部署图。
