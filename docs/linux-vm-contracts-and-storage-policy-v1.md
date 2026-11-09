# Linux VM 接口与存储治理契约 v1（设计附录）

> 本文定义未来实现，不是当前已存在的 API。所有新增名称、字段、错误码均为本地设计。
> 设计背景、模块责任及平台门槛见[主方案](G:/Code_Warehouse/DeepAgent-Studio/docs/2026-11-09_linux-vm-architecture-upgrade-report.md)。

## 1. 先保留现有入口

| 已存在的 UI/协议入口 | 升级后的处理 |
|---|---|
| Tauri `start_chat_v2` 与当前流式/审批链 | 继续调用原 ChatService/AppCore，不从前端绕开它进入 VM |
| Harness `thread/start`、`thread/resume`、`turn/start`、`turn/interrupt`、`turn/steer`、`approval/respond` | 使用同一任务目标、权限和 Kernel 运行控制 |
| Harness `config/read`、`sandbox/status` | 投影同一个环境解析结果与后端支持状态；能力声明不代表已授权 |
| Tauri `runtime_list/status/roots/install/cancel/uninstall/migrate_resources` | 保持现有参数语义和返回兼容；新增 Linux 资源按 registry ID 操作 |
| `runtime:progress` | 保持订阅兼容；新阶段通过版本化扩展表达，不改老字段含义 |
| Tauri `get_settings` / 既有设置服务 | 新 policy 进入统一设置存储；不能只存在 React localStorage |

当前 CLI 没有通用 `runtime install` 命令。实现阶段若需要增加 `environment status/prepare`、`storage status/cleanup`，必须同时扩展 CLI 参数解析、AppCore 服务与测试；这些名字不作为当前可执行指令提供。

新增 UI 展示 DTO 与 Harness DTO 分开，但数据来自同一 AppCore 操作。既有协议版本 1 不能直接接受未知 `sandboxBackend` 就假装支持；需要能力协商与兼容测试。

## 2. 最小新增 UI 管理接口

下表的拟议 commands 只供可信本地管理 UI 使用，输入由 Rust 再校验。新增 commands 的注册、权限和窗口来源都必须限定；任意网页不能调用。

| 拟议 command | 请求关键字段 | 返回/行为 |
|---|---|---|
| `execution_environment_status` | `schemaVersion: 1`, 可选 `projectId` | 平台/虚拟化、目标、基础镜像、工具版本、支持位、诊断；不返回密钥 |
| `execution_environment_resolve` | `projectId`, `targetPreference`, 工具要求 | 只读解析计划、原因及缺失项；不因此启动命令或扩大权限 |
| `execution_environment_prepare` | registry 中已知 `imageId` | 启动离线激活/准备任务并返回 `operationId`；没有任意 URL/DLL/路径参数 |
| `storage_status` | `schemaVersion: 1` | 全局 Linux 池、guest 用量、预留、可回收、租约和近期维护结果 |
| `storage_cleanup_preview` | 已知 `scope`, `reason: manual` | 有效期内的 `planId`、资源 ID、估算字节及跳过原因；无删除副作用 |
| `storage_cleanup_apply` | `planId`, `expectedGeneration` | 重验身份/租约/引用，执行允许删除的项目，返回操作 ID 和真实结果 |
| `local_execution_policy_update` | `expectedRevision`, policy | 同一 SettingsService 内校验并提交；只更新该设置片段，防止覆盖密钥/模型等并发设置 |

`local_execution_policy_update` 是拟议的局部设置更新入口，不能把它描述为当前已有 `save_settings`。若实现时既有设置更新边界已能表达条件更新，优先扩展原边界而非保留两个写入口。

环境准备与清理使用进度投影，拟议事件为 `environment:progress`、`storage:maintenance`。它们属于管理操作状态，不是另一个 run/event store；重要变化同步进入现有诊断/控制记录。清理操作 ID 只用于维护查询，不能冒充 Agent run ID。

### 2.1 环境解析结果语义

拟议 DTO 结构示意如下，只定义字段形状，不能直接当作现有接口调用：

```typescript
type ExecutionTarget = "host" | "linux_vm";
type EnvironmentState =
  | "not_prepared" | "preparing" | "ready" | "unavailable" | "needs_attention";

interface EnvironmentPlanV1 {
  schemaVersion: 1;
  planId: string;
  target: ExecutionTarget;
  reason: string;
  state: EnvironmentState;
  configSnapshotId: string;
  imageDigest?: string;
  guestArchitecture?: "x86_64" | "aarch64";
  tools: Array<{
    kind: "python" | "java" | "node" | "git";
    version: string;
    location: "host_local" | "host_managed" | "guest_image" | "guest_project";
  }>;
  missingCapabilities: string[];
}
```

解析计划是比较/展示材料，不是可无限重用的授权票据。运行准备仍要验证配置快照、项目身份、当前权限、镜像与空间租约。不同 OS 的 executable 路径不进入同一个 `RuntimeResolution.executable` 字段。

### 2.2 设置界面调用顺序示意

以下是未来调用顺序；当前尚未注册这些 commands：

```typescript
// 设计示意：只能在可信本地 Tauri 页面中调用。
const state = await invoke("execution_environment_status", { schemaVersion: 1 });
const space = await invoke("storage_status", { schemaVersion: 1 });

// 用户点击“清理可回收空间”：先给出具体清单，再应用计划。
const plan = await invoke<{ planId: string; generation: number }>(
  "storage_cleanup_preview", { scope: "linux_pool", reason: "manual" }
);
await invoke("storage_cleanup_apply", {
  planId: plan.planId, expectedGeneration: plan.generation
});
```

正常 Agent 执行入口仍是现有聊天/Harness；UI 不发送“跳过审批”“任意宿主可执行文件”“指定 VMM 库”等字段。

## 3. 与现有 Rust 类型的适配边界

| 当前类型/边界 | 拟议升级 | 兼容要求 |
|---|---|---|
| `SandboxBackendKind` | 新增 `LinuxVm` / serialized `linux_vm` | 旧值不变；枚举消费者、CLI、配置和错误测试同步 |
| `SandboxCapabilities` | 版本化展示 guest OS/arch、流式命令、PTY、网络/文件策略等能力 | 现有 one-shot/PTY 字段仍诚实返回；未测 PTY 返回 false |
| `SandboxExecutionRequest` | 通过准备上下文关联 plan、工作区 handle、策略和空间租约 | 现有 Direct/Windows 后端不得把 guest 路径当宿主路径 |
| `SandboxCodeRequest` | 显式区分 host 与 guest SDK binding | 不能把 `/usr/bin/python3` 填进一个需要真实宿主文件的字段 |
| `CommandExecutor` | 继续作为 bash 等工具的适配入口 | Kernel 的取消/deadline 不被新的固定超时替换 |
| `RuntimeArtifact/Platform` | 新增 artifact domain 和 guest target 维度；旧 artifact 默认 host | Linux guest payload 可供 Windows x64，但不是 Windows 本地 SDK |
| `RuntimeBroker` | 继续解析 host；guest 由 manifest/握手提供运行时目录与版本 | 保持 `prefer_local/local_only/managed_only` 的旧宿主含义 |
| `TerminalSessionBackend` | 实现 VM session、resize、signal 和 lease 控制 | 不改成独立 WebSocket 终端绕过原租约 |
| `ToolExecutionContext` | 透传 token、deadline、observation | 不向 guest 赋予修改根取消树的能力 |

新增 `ExecutionPlan` 属于本地执行契约，内容至少为：选定目标、所需/实际工具版本、workspace snapshot、镜像 digest、网络规则摘要、授权范围、配额预留与 capability snapshot。run 准备、展示和实际 dispatch 消费同一份计划。

## 4. 宿主与 worker 的接口

宿主向自己的 `vm-worker` 传递已验证 manifest 标识和受控资源句柄/私有启动描述。worker 不接受来自 WebView 或模型的动态库路径、系统盘路径或任意加载参数。

平台适配负责：引擎/固件 ABI 验证、虚拟化创建、固定尺寸块盘、受限只读输入设备、双向控制通道，以及退出时关闭磁盘/设备。子进程从净化后的环境启动，不继承 API Key、SSH、代理凭据和宿主开发工具 PATH。

`krun_start_enter` 的进程接管行为留在 worker；AppCore 通过监督器持有进程、租约和取消句柄。生产进程退出必须回收后代；工作文件状态不能代替真实进程存活检查。

Windows 双向通道的首选设计是独立 virtio-console 控制端口。Demo 只验证过默认 console 的 HANDLE 输出路径；multiport、输入 HANDLE、断线及大流量仍需 P0 验证。精确导出名称和签名以锁定二进制所对应的头文件为准，不能照抄另一个版本的 libkrun `main`。

若该通道失败，可以评估同一安全属性的替代传输并重新验收，但不能降级为“guest 写一个 JSON 文件就有宿主权限”或公开的无认证本机 TCP 服务。

## 5. Linux 控制协议 v1

这是独立的宿主/guest 内部协议版本，不能与 Harness machine protocol 的版本混淆。设计使用有长度的帧；控制与大数据通道分开，避免 stdout 伪造控制消息。

| 层 | 规则 |
|---|---|
| framing | 固定长度头表达帧大小；控制帧最大 1 MiB，文件/输出数据块默认最大 64 KiB |
| handshake | `protocolVersion`、host 签发的 boot generation、guest arch、镜像和组件版本、实际能力 |
| correlation | `requestId`、宿主创建的 `runId/actionId/execId`、`bootId`；不接受越权复用 |
| ordering | 每个 exec 的输出序号单调，拒绝重复/过期 generation；宿主生成最终持久化事件 sequence |
| flow control | 有界接收队列、文件/输出字节预算、背压、明确截断和 EOF |
| failure | 超长帧、非法状态、未知动作或跨 scope ID 被拒绝，必要时隔离该 worker |

拟议控制方法：

| 方法 | 请求关键项 | 响应/事件 |
|---|---|---|
| `guest/hello` | 启动 generation 和版本协商 | 能力与工具探测；不自行扩大配置 |
| `exec/start` | exec ID、argv 或 scriptRef 二选一、guest cwd handle、净化 env、限额、授权上下文 ID | 已启动/拒绝；只承认实际创建的进程 |
| `exec/signal` | exec ID、signal、有效 generation | 信号结果；不把 ACK 当作已退出 |
| `exec/output` | exec ID、stream、序号、数据块 | 宿主受预算约束的接收确认 |
| `exec/exited` | exec ID、真实子命令退出/信号、文件候选摘要 | 宿主校验后投影命令结果，Kernel 决定 run 终态 |
| `workspace/import` | 来源 handle、相对路径、长度和基线摘要 | 按需复制到 Linux，原始宿主路径不由 guest 决定 |
| `workspace/changes` | scope 与基线版本 | 新增/修改/删除候选，宿主受控获取内容 |
| `resource/usage` | volume/scope ID | 块、inode、缓存和进程观察；硬上限仍由宿主有界设备保证 |
| `cache/prune` | 宿主已审核的候选 ID/预算与无活动租约条件 | 实际 guest 可用块变化及失败项 |
| `network/request` | 已授权连接/请求范围与数据流 ID | 宿主代理执行；不能自选任意通道连接宿主服务 |
| `terminal/*` | 既有 terminal session 与输入 lease 的投影 | PTY 数据/resize/signal；阶段验收后再协商为支持 |

协议中的策略 ID 只用于查找宿主已保存的权限事实，guest 不能靠填写 `approved: true` 自己批准操作。即便 guest 被完全控制，宿主网络、路径、写配额与唯一终态边界仍要成立。

脚本原文不经过宿主 shell 转义。控制服务通过 scriptRef 获取有界字节、校验摘要、落盘，然后以固定 guest 解释器执行脚本文件。不能在 libkrun 内核启动参数中嵌入任意长、含引号的用户代码。

## 6. 三种文件能力

| 能力 | 默认规则 | 宿主授权与审计 |
|---|---|---|
| read/import | 仅授权 workspace/指定文件，受输入大小与名称检查 | 来源 handle、路径摘要、基线 hash、实际导入数量 |
| propose/export | 默认工作副本只提交变更提案 | 原目标授权、观察版本、链接身份、原子发布、冲突与实际产物 |
| direct writable mount | 默认关闭，独立高级能力 | 必须限定根与写范围；不能宣称每次写都已逐文件审批 |

导出校验至少覆盖：`..`、绝对路径、UNC/device 路径、Windows ADS 与保留名、大小写冲突、控制字符、软链接/junction/hardlink 风险、链接替换、异常大文件、重复条目、既有文件变化及取消状态。

清理器不因一个路径名叫 `tmp`、`cache`、`node_modules` 就获得删除权。只有受管归属、资源类型、引用、租约及内容身份都满足策略时才清理。

## 7. 错误与运行终态

下列为拟议机器错误码，需随实现加入契约测试：

| 错误码 | 触发 | 运行语义 |
|---|---|---|
| `EnvironmentUnavailable` | 无可用虚拟化/目标 | 不派发，不自动转宿主 |
| `RuntimeRequirementUnsatisfied` | 缺工具或版本不兼容 | 返回实际版本与要求 |
| `EngineIntegrityMismatch` | 镜像、库或 manifest 校验失败 | 不加载或启动 |
| `VmProtocolMismatch` | ABI/协议/generation 不匹配 | 中止准备，报告诊断 |
| `QuotaExceeded` | 新预留无法容纳 | 不开始新写入，返回占用与受保护资源 |
| `GuestFilesystemFull` | guest 块或 inode 用尽 | 命令失败，保留有界输出和审计 |
| `WorkspaceConflict` | 宿主文件已改变 | 不覆盖，展示精确冲突 |
| `PathDenied` / `NetworkDenied` | 路径/目的地址未授权 | 明确拒绝，不要求 guest 自行绕过 |
| `ArtifactExpired` | 原文按策略回收 | 历史仍可查看元数据，不伪装为空输出 |
| `ResourceBusy` | 活动租约或并发维护 | 排队或返回忙，不删除文件 |
| `AuditUnavailable` | 关键授权/提交审计无法保存 | 拒绝新的高风险操作；保留恢复信息 |

Linux 命令退出 23 可以是正常执行完成的命令结果，不能改为 VM 进程退出 0。run 是否成功由现有工具/Kernel 规则决定。用户取消、deadline、worker 崩溃和输出截断也必须区分；是否终止输出洪泛任务由统一资源策略决定，普通日志达到保留上限不应默认把已完成命令改成成功或取消。

原始输出达到容量后，默认保留截断标记并继续有界 drain/丢弃多余字节，控制消息不与它争用无界队列。不能简单停止读取管道导致子命令死锁；超过速率或 CPU 等资源策略时才经 Kernel 明确终止并记录原因。

guest 取消确认不是终态；要等待进程实际退出。若进程状态不确定，保持资源租约并走现有恢复/终态协调，不能把控制文件缺失当成“已清理”。

## 8. 可配置默认值

以下 JSON 是拟议 `AppSettings.local_execution` 的有效策略样例，不是当前应用已支持的配置文件。后端需要增加 schema version、默认填充、条件更新和上下界校验。不得将这些字段直接拼进命令行。

```json
{
  "schema_version": 1,
  "target_preference": "prefer_compatible_host",
  "linux_vm": {
    "enabled": true,
    "max_running_vms": 1,
    "idle_shutdown_seconds": 180,
    "default_vcpus": 2,
    "default_memory_mib": 1536,
    "network_default": "disabled",
    "workspace_mode": "copy_then_commit"
  },
  "storage": {
    "max_linux_pool_bytes": 3000000000,
    "max_writable_volumes_total_bytes": 1000000000,
    "vm_raw_log_budget_bytes": 100000000,
    "per_execution_output_bytes": 8388608,
    "temporary_host_files_budget_bytes": 300000000,
    "guest_cache_target_bytes": 250000000,
    "guest_pressure_ratios": [0.70, 0.85, 0.95],
    "guest_cleanup_target_ratio": 0.60,
    "host_pressure_ratios": [0.85, 0.92]
  },
  "cleanup": {
    "periodic_enabled": true,
    "pressure_enabled": true,
    "interval_minutes": 15,
    "startup_delay_seconds": 30,
    "post_run_debounce_seconds": 60,
    "usage_sample_seconds": 5,
    "successful_raw_logs_days": 3,
    "failed_raw_logs_days": 7,
    "finished_scripts_minutes": 30,
    "recoverable_workspace_hours": 24,
    "unused_package_cache_days": 7,
    "abandoned_download_hours": 24,
    "unreferenced_image_grace_hours": 24,
    "audit_summary_days": 90,
    "preserve_active_and_pinned": true
  }
}
```

其中 cache target 是 data 卷内部的策略值，不是额外获得 250 MB 宿主空间。base、seed、worker/engine 的已验证 manifest 大小与日志/下载/数据卷预留共同约束总池，不把各子预算都独立发满后再去检查总量。

### 8.1 字段约束与生效时机

| 设置 | UI 可调整范围/规则 | 生效时机 |
|---|---|---|
| 总池上限 | 可下调，不超过 3,000,000,000；不低于已承诺且不可回收部分 | 先验证可达；不能清理到目标则拒绝降低，不假装生效 |
| 全部可写卷容量 | 分项目卷合计不超过总池允许分配；改变已有卷需停机迁移 | 不能截断一个已有 ext4 文件来缩容 |
| VM 并发 | 首期固定 1；并发执行/信任域隔离通过后才开放 | 下一次调度；跨项目合计仍受全局配额 |
| CPU/内存 | 按宿主能力和产品允许范围校验 | 下一次 VM 启动；磁盘 3 GB 不等于内存限额 |
| 保留天数 | 原始日志可 0–30 天；摘要默认 90，可 7–365 天 | 不缩短当前用户固定/必要历史引用的寿命 |
| 清理周期 | 默认 15 分钟，可 1–1440 分钟 | 下一次维护轮；硬配额始终有效 |
| 压力阈值 | 严格递增、小于 1；cleanup target 小于最低压力阈值 | 新策略版本生效；当前操作持有旧快照直到安全切换 |
| 清理开关 | 可关定时或压力清理 | 两者都关时，满额拒绝新写入，不能绕过硬配额 |
| 网络默认值 | 默认禁网；批准代理仍需任务授权 | 新 run；不能给运行中任务静默增加出网范围 |
| 工作区模式 | 默认 copy/commit；直接写映射需高级能力和授权 | 新任务；不改变已批准目录的权限 |

配置修改进入审计：修改者、前后摘要、revision、来源和时间。项目配置不能扩大机器级最大空间、禁删规则、宿主目录权限或网络边界。冲突时使用现有配置层级/信任规则，不额外制造一套高优先级 JSON 文件。

任务 command timeout 继续由现有 `ToolExecutionContext` / Kernel 配置决定；这里不再增加一套独立的“VM 命令超时默认值”。启动、握手和停机宽限期是不同控制阶段，也必须服从父任务总 deadline。

## 9. 资源目录、下载与安装契约

资源安装位置遵循 `UserPaths` 和 RuntimeService 活动根。每项 VM 专用资源必须具有不可变 ID、类型、受管根引用、相对路径、版本/digest、大小上界及归属；下载/安装资源继续使用现有 InstallLedger，不维护相互矛盾的来源副本。离线激活派生到用户数据根的镜像/卷也要登记归属，不与安装目录中无需自行卸载的 seed 混淆。

1. UI/模型只能请求受信 registry ID，不能传任意下载 URL 或镜像启动参数。
2. `RuntimeService` 当前临时文件为活动根下 `.dl-<id>.tmp` 或 installer 对应 `.dl-*`。迁移到带 install ID 的暂存时需保留旧临时文件的安全识别与清理，不能全局按 `.tmp` 删除。
3. 下载前预留最大压缩字节和后续展开预算；实际流量超过已批准上界立即停止。没有 Content-Length 不能被理解为无限大小。
4. VM 链要求所有引擎、镜像与固件具备固定摘要及受信发布身份。不能继承当前其他 runtime 的 `Installer` 或 `allow_unpinned_files` 例外放宽 VM 校验。
5. 使用独立 staging，检查路径、链接、单文件和总展开大小、文件数；完成校验后原子发布到 digest 目录，最后写安装归属及 active 引用。
6. cancellation、校验失败和断电后留下可识别的安装状态；只有无活动安装租约的残留才可清理，不能误删另一个下载的临时文件。
7. 更新 staging、旧/新 active 和回滚资源同时计入池；全流程无法容纳就拒绝/延期，绝不能只检查最终大小。
8. Linux 项目依赖位于 guest data 卷；即使未来把包下载缓存放宿主，也计入 Linux 池，并按 OS/arch/ABI/源/锁文件隔离。

应用主安装目录内的压缩 seed 由安装器管理，清理器不擅自删 Program Files/应用包资源。若改为首次激活后移除 seed 的另一种安装方式，必须明确卸载/修复/离线重建行为并重新测峰值；不能在当前预算里提前假定它已经删除。

## 10. 定时与压力清理状态机

默认调度由 AppCore Rust 服务负责，所有触发源只提交维护请求：启动补做、15 分钟定时、任务结束、配额压力、手动清理。调度器合并重复请求，最多一个维护执行者；UI 断开不影响正在执行的安全维护。

```text
等待触发
  → 取得明确的跨进程维护锁
  → 拍摄策略/资源 generation 快照
  → 枚举有归属的候选并检查引用、任务与安装租约
  → 生成受限清理计划
  → 逐项再次验证身份、generation 和实际存活状态
  → 记录删除意图并执行
  → 核对回收量，更新同一产物/安装/预留索引
  → 记录已删/跳过/失败并释放锁
```

不是只有时间到才允许清理；默认压力策略可以提前淘汰已终止任务的可丢弃原文和缓存。Pinned、活动安装、尚未提交的必要产物和恢复租约优先；所有候选受保护时明确清理不足，不继续扩大删除范围。

只依据持久化 `state=running` 既不能证明资源仍存活，也不能证明可删。协调 Kernel 的 durable 状态、worker generation、进程句柄和租约；无法证明安全时保留并报告。失效租约回收不通过 PID 单独判断，避免 PID 重用。

### 10.1 清理优先级

1. 已失效且无安装租约的下载/解压残留、已提交的输出暂存、已终止任务的临时脚本。
2. 无活动引用的包下载缓存、受管临时 HOME 和可重建编译中间产物。
3. 超过保留期的完整原始日志；压力下按明确策略提前淘汰可丢弃内容。
4. 没有活动/恢复/回滚引用的旧镜像、环境和可回收卷。
5. 用户主动管理的固定资源，通过清理预览展示，不能默认纳入自动删除。

不清理用户原项目、用户选定的交付文件、凭据、系统开发环境、其他软件缓存和未知文件。清理“依赖缓存”也不能直接递归删除正在使用的 venv/`node_modules`。

### 10.2 历史引用和 TTL 如何兼容

现有 `gc_tool_results_before` 保护被保留历史引用的工具结果，不能为了日志 TTL 简单绕开它。实现时需要明确两类内容：

- **必要工具结果**：模型/会话 replay 所需的有界结果，继续受现有引用保护。
- **原始执行日志附件**：完整 stdout/stderr、脚本原文等可过期大内容，具有显式 `available/expired` 的版本化内容状态；过期后保留 artifact ID、摘要、hash、时间和清理原因。

对原始附件执行过期时更新状态并保留 tombstone，而不是让历史引用指向不存在却声称可读取的文件。具体 ArtifactStore 扩展要与现有删除意图、身份验证和崩溃重试协议合并实现。历史事件保持顺序和语义稳定，不能改写旧 sequence。

用户固定日志仍受容量约束。若固定项占满池，暂停新的高风险执行并展示保留项；不得通过悄悄删其他工作区或扩大 3 GB 上限解决。

### 10.3 数据库与真实释放空间

RuntimeLogStore 的 TTL/容量清理由它的既有写入线程负责，采用分批删除和经过验证的维护策略。不能由前端删除整个 `runtime-logs.db`，也不能在活跃聊天时强行 VACUUM 整个主数据库。

SQLite 删除记录与磁盘文件缩小也需要分开统计：已回收逻辑数据、文件系统可用空间和文件长度分别返回。数据库压缩、raw 卷重建、qcow2 回收都不能在未验证时承诺“删除多少立刻给宿主还多少”。

系统时间回拨、应用休眠及错过多个周期时，采用安全的截止时间计算和合并补做；不把未来时间文件批量认定为过期。后台任务失败采用有限退避并记录原因，不能每次 UI 刷新都重启一遍清理。

## 11. 空间与清理状态 DTO

拟议状态至少返回下列维度，避免把一个含糊的“Linux 占用”数字当作全部事实：

| 字段 | 含义 |
|---|---|
| `hardLimitBytes` | 默认且不超过 3,000,000,000 的总池硬限额 |
| `committedBytes` | 已发布资源计量上界，固定 raw 卷按容量计 |
| `inflightReservedBytes` | 未完成下载/安装/导出/写入已预留金额 |
| `reclaimableHostBytes` | 预计删除受管文件可回收的宿主空间 |
| `guestUsedBytes/guestAvailableBytes` | guest 块用量；不是再次累加到 committed 的空间 |
| `guestInodesUsed/guestInodesAvailable` | 小文件耗尽诊断 |
| `protectedBytes` | 被租约、引用、回滚或用户固定的资源 |
| `categories` | seed、engine、base、volume、raw log、staging 等分类 |
| `lastMaintenance/nextMaintenance` | 原因、策略 revision、实际开始/结束、结果和下一次计划 |
| `pressureState` | normal、cleaning、admission_blocked、needs_attention |

清理结果逐项包含资源 ID、类型、预计/实际回收量、状态和跳过原因。`estimatedBytes` 不能冒充 `reclaimedBytes`；删除失败也不能先从配额账本扣除。

## 12. 一次具体任务的接口路径

以“在 Linux 中运行 Python 脚本，读取项目 CSV 并生成报告”为例：

1. 用户通过现有聊天入口提交任务并指定 Linux。准备阶段解析 Linux Python 版本、输入文件权限和输出目标。
2. 既有审批机制批准所需读/写/联网范围；本地 CSV 处理不申请额外网络。
3. 宿主生成 `execId` 和资源租约，按文件观测版本导入 CSV。脚本原文按字节传输，记录摘要。
4. guest 在 `/data/workspaces/<scope-id>` 用 guest Python 执行。输出经受限数据流实时回到 Rust，再投影给 UI/模型。
5. guest 申报候选报告。Rust 验证格式、长度、相对路径、导出目标和用户并发修改，按现有文件提交协议发布。
6. Kernel 记录唯一终态，ArtifactStore 提供报告引用。用户最终报告保留在授权目录，不能被 VM 垃圾清理器删除。
7. 30 分钟后可回收临时脚本/工作副本；3 天后完整成功日志可过期。命令摘要、退出状态、实际产物、版本和清理记录仍有迹可循。

如果该任务读取量或依赖超过剩余配额，返回 `QuotaExceeded`，不擅自改在宿主运行、不导入全部用户目录，也不清掉用户原 CSV。
