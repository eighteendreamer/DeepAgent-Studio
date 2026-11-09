# Linux VM 升级方案：证据、要求覆盖与验收索引

> 调研日期：2026-11-09；对象为当前 DeepAgent Studio 工作区和 `G:\Code_Warehouse\DeepAgent-VM-Lab\tauri-demo`。
> 本文证明方案有代码/实测依据，不证明未来生产实现已完成。主项目存在其他任务的并行改动与提交，本轮仅新增本方案文档。

## 1. Evidence：当前代码事实

以下文件可用 `Get-Content` 读取表内的实际绝对路径，并通过对应 Rust 标识搜索定位。表内链接为本次工作区路径，不是外部项目的业务代码复制来源。

| ID | 证据位置 | 观察与限制 |
|---|---|---|
| E01 | [data_root.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/data_root.rs) | `UserPaths` 定义统一数据布局，默认 `~/.deepagent`，运行时位于 `files/runtimes`；原数据库/显式环境覆盖规则需兼容 |
| E02 | [sandbox_backend.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/sandbox_backend.rs) | 当前只有 Direct、Sandboxie、WindowsSandbox；提供 execute/execute_code 和能力，显式隔离后端失败不退回 Direct |
| E03 | [runtime_service.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/runtime_service.rs) | RuntimeService/Broker、宿主解析与三种 preference；RuntimeKind 当前无 Git；有版本/完整性例外，VM 不能不加区分继承 |
| E04 | [api.ts](G:/Code_Warehouse/DeepAgent-Studio/apps/desktop/src/api.ts)、[Tauri lib.rs](G:/Code_Warehouse/DeepAgent-Studio/apps/desktop/src-tauri/src/lib.rs)、[ConfigSettings.tsx](G:/Code_Warehouse/DeepAgent-Studio/apps/desktop/src/components/settings/ConfigSettings.tsx) | 现有 runtime 管理 commands、`runtime:progress` 和设置页；Rust 端业务授权仍需贯彻 |
| E05 | [run_assembler.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/run_assembler.rs)、[execution_config.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/run_assembler/execution_config.rs)、[prepared_chat.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/run_assembler/prepared_chat.rs) | 按 run 准备执行器/快照；现有 backend executor 默认禁网，网络升级不能只改 UI |
| E06 | [fs_guard.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-builtins/src/fs_guard.rs)、[file_mutation.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-builtins/src/file_mutation.rs) | 真实文件身份、路径门禁、先读后改、陈旧读取和路径提交锁；导出应复用此边界 |
| E07 | [terminal_service.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/terminal_service.rs)、[terminal lib.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-terminal/src/lib.rs) | 当前本地 PTY 与共享终端/输入租约；不是已存在的 VM PTY 实现 |
| E08 | [run_store.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-persistence/src/run_store.rs)、[runtime_event_log.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/runtime_event_log.rs)、[runtime_log_store.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-persistence/src/runtime_log_store.rs) | 唯一运行终态、事件持久化、日志脱敏和独立日志写入者；当前未见完整通用日志容量/TTL 调度 |
| E09 | [archive_service.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/archive_service.rs)、[service.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/service.rs)、[tool_result_retention.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-persistence/src/tool_result_retention.rs) | 启动时 30 天回收站 GC；保留历史引用的工具结果；不是可随意改成按日志年龄删文件 |
| E10 | [managed_files.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/managed_files.rs)、[managed_file_store.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-persistence/src/managed_file_store.rs) | 盘点当前使用 metadata length，跳过符号链接；不是并发空间预留或完整硬配额 |
| E11 | [install_ledger.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/install_ledger.rs)、[hooks.nsh](G:/Code_Warehouse/DeepAgent-Studio/apps/desktop/src-tauri/installer/hooks.nsh) | 来源/位置/卸载归属已有统一记录；卸载通过 `deepagent*.exe` 查找 CLI，新增同名前缀 worker 需要处理 |
| E12 | [http_runtime.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/http_runtime.rs) | typed HTTP broker 已有公共地址与逐跳重定向校验；不等于完整 guest TCP 代理 |
| E13 | [workflow_code.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/workflow_code.rs)、[hook_runtime.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/hook_runtime.rs)、[runtime_payload.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-app-core/src/runtime_payload.rs) | 工作流 SDK 依赖 RuntimeBroker；插件 payload/hook 启动同样需执行域审查，不能只迁 bash |
| E14 | [requests.rs](G:/Code_Warehouse/DeepAgent-Studio/crates/deepagent-harness-protocol/src/requests.rs)、[CLI args.rs](G:/Code_Warehouse/DeepAgent-Studio/apps/cli/src/args.rs)、[tauri.conf.json](G:/Code_Warehouse/DeepAgent-Studio/apps/desktop/src-tauri/tauri.conf.json)、[release workflow](G:/Code_Warehouse/DeepAgent-Studio/.github/workflows/release-desktop.yml) | 当前 Harness v1 和实际方法、CLI 实际子命令、打包资源、CSP/asset 范围及发布平台矩阵 |

当前本地参考中，[DeepSeek Harness Desktop 文档](G:/Code_Warehouse/DeepAgent-Studio/借鉴/deepseek-harness/apps/desktop/README.zh.md)说明私有运行时、资源和受限 preload 的边界，可借鉴其“前端不直接获得 shell/任意安装参数”的设计。它不是本项目 VM、JDK 或 Python 全部随包的证明。更高优先级的本项目同类实现已经覆盖资源/执行入口，无需从参考项目复制业务链。

## 2. Evidence：Demo 已证明什么

| ID | 实测/源码位置 | 可支持的结论 |
|---|---|---|
| E15 | [engine.rs](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/src-tauri/src/engine.rs)、[runner.rs](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/src-tauri/src/runner.rs)、[guest supervisor](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/guest_supervisor.py) | 直接 Windows libkrun C ABI、独立 worker、ext4/qcow2、净化环境、宿主日志；没有运行时调用 WSL/Docker |
| E16 | [native-suite.json](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/ui-data/evidence/native-suite.json)、[frontend receipt](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/ui-data/evidence/frontend-receipt.json)、[结果截图](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/evidence/tauri-native-results.png) | 19 项通过、1 项未运行；Tauri invoke/事件与 20 项 DOM 结果的真实链路。无 WSL2 干净系统明确未验收 |
| E17 | [空间探测记录](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/.cache/usage-probe/evidence/quick-native.json) | 使用新写入层读取基础镜像用量；命令约 749 ms，不是完整产品启动性能基准 |
| E18 | [安装包](<G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/src-tauri/target/debug/bundle/nsis/DeepAgent VM Lab_0.1.0_x64-setup.exe>)、[构建记录](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/evidence/tauri-bundle-final.log)、[主桌面历史构建记录](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/evidence/main-desktop-build.log) | Demo NSIS 构建成功及大小、历史桌面构建成功；不是干净安装、全部产品/全部平台通过 |

空间数据（字节，来自 E17）：

| 指标 | 字节数 |
|---|---:|
| 当前基础磁盘文件 | 2,147,483,648 |
| ext4 统计总块容量 | 2,040,373,248 |
| 已用块 | 781,082,624 |
| 空闲块合计 | 1,259,290,624 |
| 普通进程可用块 | 1,135,140,864 |
| 空闲中预留部分 | 124,149,760 |
| 文件大小与统计块容量之差 | 107,110,400 |

预留部分是空闲块的子集；不能再次相加。上述数值不是缩容后的测量结果。当前安装包为 313,237,475 字节，压缩镜像为 307,318,610 字节；展开后约 2.53 GB 的实验目录包括程序/压缩副本、2 GiB 基盘、约 25 MB 写入层与少量日志。

本轮重新读取的引擎摘要，与旧实验 manifest 一致：

```text
krun.dll
1916bcfcb3d0852b49908d9b2077f3c0100c366f6ed2acf26e988e90c816cb2b
libkrunfw.dll
19e20d939dcab8c52f8c973e1e01fdc3be88fd1922e893488c33d71a3770524a
```

复核命令如下。摘要对应仅证明文件身份，不证明全部 ABI、来源 commit 或安全属性已经验收。

```powershell
Get-FileHash -LiteralPath 'G:\Code_Warehouse\DeepAgent-VM-Lab\tauri-demo\runtime\engine\krun.dll','G:\Code_Warehouse\DeepAgent-VM-Lab\tauri-demo\runtime\engine\libkrunfw.dll' -Algorithm SHA256
```

Demo 的 19 项覆盖 Linux 身份、Python/pip/venv、Java 编译/JAR、Node/npm 离线脚本、Git 提交/diff、POSIX 文件语义、只读输入/输出回传、重启持久化、UTF-8 流、退出码、deadline、取消、恢复、输出上限、禁网/指定本机服务、基础镜像未变和宿主读取产物。它未证明严格 3 GB、生产只读 root/raw 数据卷、非 root、全部公网协议、全量 PTY、动态授权、完整卸载或恶意代码防逃逸。

## 3. 官方依据及适用范围

| ID | 官方/一手资料 | 用于本方案的事实 |
|---|---|---|
| E19 | [smolmachines 支持平台](https://smolmachines.com/docs/introduction/concepts/supported-platforms) | Windows 原生 WHP 与 WSL/KVM 是不同路径；宿主/guest 架构及平台支持有约束 |
| E20 | [WHvGetCapability](https://learn.microsoft.com/en-us/virtualization/api/hypervisor-platform/funcs/whvgetcapability) | Windows hypervisor 能力可探测；仍需完成实际 VM 启动验证 |
| E21 | [Tauri resources](https://v2.tauri.app/develop/resources/)、[Tauri capabilities](https://v2.tauri.app/security/capabilities/) | 资源目录解析与窗口能力边界，不代替应用权限 |
| E22 | [libkrun README](https://github.com/libkrun/libkrun/blob/main/README.md)、[安全模型](https://raw.githubusercontent.com/libkrun/libkrun/main/docs/security-model.md) | ABI 版本风险、设备代理的宿主权限；不得把 VM 当作无限可写共享的安全保证 |
| E23 | [Linux ext4](https://www.kernel.org/doc/html/latest/filesystems/ext4/overview.html)、[cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) | 块容量与资源控制机制；本次未证明所发 guest 内核具有全部控制器 |

以上文档不是“本机已测试通过”的替代。优先使用当前仓库/Demo 的结果；本地新接口、配额算法、代理与目录方案明确为设计。

## 4. Finding → Path：为何这样升级

| Finding | 证据 | 架构决策 | 实施路径 |
|---|---|---|---|
| F01：已有统一运行链足以接入新目标 | E02、E05、E08、E14 | 增加后端和执行计划，不另造 Agent/事件/审批系统 | P2，验收 V05/V06 |
| F02：现有 RuntimeBroker 是宿主解析器 | E03、E13、E15 | 目标先选、工具再解析，host/guest 路径分离 | P2/P3，V04/V07 |
| F03：日志已可保存在宿主，清理尚不完整 | E08–E10、E15 | 扩展现有存储生命周期与定时维护 | P4，V12–V15 |
| F04：Demo 大部分基盘空间为空闲 | E17、E18 | 生产打样缩容；有界 raw 卷避免增长失控 | P0/P1，V08/V09 |
| F05：guest 输出/路径不是宿主授权事实 | E06、E12、E15、E22 | 宿主文件提交与网络策略保持最终权威 | P2/P3，V10/V11 |
| F06：并发与升级峰值会绕过简单容量轮询 | E10、E17 | 全局原子预留 + 有界设备 + 拒绝超限 | P1/P4，V08/V13/V16 |
| F07：原生成功与全平台产品成熟度不同 | E15–E20、E22 | 按支持位和平台分阶段发布，失败不退宿主 | P0/P5，V01–V03/V17 |

P-call-01（命令与文件调用路径）：用户 → 现有 Tauri/Harness → RunAssembler 配置/审批快照 → Kernel 工具执行 → SandboxBackendCommandExecutor → Linux VM 后端 → 受控 guest 命令 → 宿主输出/网络/文件边界 → 原 RunStore/ArtifactStore。关联 E02、E05、E06、E08、E12、E15；剩余风险为 P0–P5 未实现部分。

P-storage-01（资源回收路径）：默认设置或压力事件 → AppCore 维护锁 → 既有资源归属和引用 + 新增空间/租约控制 → 校验实际存活与路径身份 → 原删除意图/提交协议 → 更新索引和回收审计。关联 E01、E08–E11；不能将现有 60 秒盘点误认为已经实现此路径。

## 5. 用户要求覆盖检查

| 要求 | 方案位置 | 完成含义 |
|---|---|---|
| 深入结合项目与 Demo | 主方案 §2、证据 E01–E18 | 有当前实现、实验和缺口映射 |
| 完整模块/框架 | 主方案 §3–§6 | 有责任、既有边界、迁移对象与信任关系 |
| 前端、Rust、Linux 如何交互 | 主方案 §9、接口附录 §1–§5 | 有时序、消息与唯一状态权威 |
| 命令/脚本执行 | 主方案 §9/§16、接口附录 §5/§7 | 有 argv/脚本、输出、取消、异常和资源控制 |
| 操作用户文件 | 主方案 §10、接口附录 §6/§12 | 有读入、工作副本、冲突、授权和提交 |
| 接口怎么调 | 接口附录 §1–§7/§12 | 现有与拟议 API 清楚区分，含调用顺序与错误 |
| 既有开发环境安装能力 | 主方案 §2/§5/§6、接口附录 §3/§9 | 保留宿主管理，补 guest 平台维度和版本要求 |
| 安装后各类目录 | 主方案 §7/§8 | 程序、随包资源、数据根、VM 内部及用户项目分开 |
| 下载资源位置与安全 | 主方案 §7/§8、接口附录 §9 | 有 registry、暂存、哈希、展开峰值和迁移 |
| 日志/脚本/临时文件清理 | 主方案 §13/§14、接口附录 §8–§11 | 有分类、TTL、容量、引用和 tombstone |
| 定时与用户可改默认设置 | 主方案 §13、接口附录 §8/§10 | 有周期、生效、校验、应用关闭语义和开关 |
| 联网及跨边界授权 | 主方案 §10/§11/§15/§16 | 有代理、目的地址、密钥、挂载和 UI 边界 |
| Linux 最大 3 GB | 主方案 §12、接口附录 §8/§11 | 有字节口径、聚合预算、硬控制与不能保证的边界 |
| 阈值后自动清理 | 主方案 §12.4/§13、接口附录 §10 | guest 与宿主压力分开，拒绝超限且不删受保护数据 |
| 操作可追溯 | 主方案 §14、接口附录 §7/§10.2 | 命令/权限/版本/网络/产物/清理摘要与审计限度 |
| 后续升级落地 | 主方案 §17/§18、本附录 §6 | 阶段、平台、兼容性与验收门槛完整 |

## 6. 未来实现验收矩阵

这里列的是必须新增/执行的验收，不是本轮通过清单。

| ID | 场景/真实输入 | 必须观察的结果 | 当前证据状态 |
|---|---|---|---|
| V01 | 干净 Windows x64，无 WSL2/无 Docker/无四套工具 | 安装器部署后离线激活，四套工具可执行；无 WSL 调用 | 未验收 |
| V02 | 虚拟化关闭、组件缺失、重启待完成、普通用户 | 明确不可用/需要操作，不退宿主、不修改系统设置 | 未完整验收 |
| V03 | Linux/KVM、Apple Silicon、Intel Mac、Windows ARM64 | 支持/不支持准确，架构不匹配拒绝 | Linux 历史嵌套实验；其余未验收 |
| V04 | 本机缺失、版本冲突、部分工具匹配、用户显式目标 | 按任务确定目标，不能跨 OS 混用依赖或绕过权限 | 当前解析器有基础，VM 选择未接入 |
| V05 | UI→Kernel→VM 的真实命令与审批、断线 replay | 同一 run/action，权限一致，事件有序且唯一终态 | Demo UI/VM 部分已实测；Kernel→VM 未接入 |
| V06 | CLI/SDK/Harness 同一任务夹具 | 同一执行与权限结果，不产生第二套存储/取消 | 原协议有基础；VM 未接入 |
| V07 | Python/JDK/Node/Git、workflow、hook、MCP、PTY | 真实开发任务；未支持能力不宣称可用 | 四套工具 Demo 已实测，其余需集成 |
| V08 | 并发下载/日志/导入/导出，新写刚好越过 3 GB | 所有写者原子预留并拒绝越界，数据不被误删 | 新硬配额未实现 |
| V09 | guest 块/inode 写满、根只读、试图增大盘文件 | 数据卷边界生效，控制与审计仍可完成 | 生产 raw/只读/非 root 未验收 |
| V10 | 用户并发编辑、链接替换、UNC/ADS/大小写/冒号名 | 不越界、不覆盖冲突；返回精确错误 | 现有 fs_guard/部分 POSIX 基础；VM 提交未接入 |
| V11 | 禁网、代理、DNS 重绑定、跳转、IPv6、私网与宿主端口 | 无直连绕过，策略逐连接生效，敏感认证不泄露 | Demo 禁网/指定本机服务已测；完整网络未验收 |
| V12 | UI 关闭、休眠、时间回拨、错过多个定时周期 | Rust 调度可持续、合并补做、截止时间安全 | 定时治理未实现 |
| V13 | 正在安装/运行/PTY/恢复的资源遇到 GC | 保留活动租约，过期 PID/目录替换不会误删 | 原回收/租约基础已有；VM 需新增测试 |
| V14 | 原始日志 TTL 到期且会话仍存在 | 原文过期但摘要/引用状态可读，replay 不出现假结果 | 附件过期状态未实现 |
| V15 | 清理意图落库后崩溃、删除失败、应用重开 | 可重试；回收量依据真实文件，状态不提前扣账 | 既有 ArtifactStore 有基础；VM 需接入 |
| V16 | 新旧镜像 + 压缩下载 + data 卷升级峰值 | 全流程不超额，空间不足延后，旧版本可继续用 | 尚未验收 |
| V17 | 安装、更新、回滚、卸载、资源迁移与自定义根 | 不误删用户数据，剩余资源可识别/重试 | Demo 包构建成功，完整生命周期未验收 |
| V18 | exit 23、信号、子进程、deadline、worker/监督器崩溃 | 命令退出码真实，后代回收，终态只落一次 | Demo 部分实测；生产监督/Kernel 仍需测试 |
| V19 | 中文/二进制/非法 UTF-8、洪泛输出和断线 | 有界接收/写入、准确 cursor/truncation，不把输出当控制帧 | Demo 部分实测；生产双向协议未验收 |
| V20 | guest 伪造授权、跨 workspace/exec ID、恶意控制帧 | 宿主拒绝，不能改变权限、配额或用户文件 | 生产协议未实现 |
| V21 | 普通 npm/pip/Maven/Gradle/Git 联网与企业证书 | 每个客户端真实遵循策略；失败不放开直连 | 未完整验收 |
| V22 | 原 workspace、桌面 types/build 与现有终端/审批回归 | 原内核不退化，新增事件/配置兼容 | 需在实现各阶段重新运行 |

实现阶段按当前仓库配置运行相应格式、crate/workspace、桌面构建与真实平台验收。本轮文档修改不执行这些未来测试，不用空 mock 或脚本返回 0 替代实际 VM/文件/网络结果。

## 7. 历史回归结果如何使用

独立实验期间的 [workspace 历史记录](G:/Code_Warehouse/DeepAgent-VM-Lab/tauri-demo/evidence/kernel-workspace-clean-env.log)按全目标统计曾有 5,071 通过、5 失败、179 忽略。两个构建生命周期项、SDK 隔离环境项和 MCP 项随后各自重跑通过；这些结果只描述当时测试环境/代码，不是本轮当前 HEAD 的完整回归结论。

本次没有为写方案重跑整个 workspace，也没有声称剩余历史失败已被修复。并行任务仍在修改/提交主仓库，因此实施开始前应固定新的 baseline，再把新增 VM 测试与原回归分开报告。

## 8. 文档本身的验收

本轮文档已完成：本地文件链接存在、UTF-8 与 fenced code 检查、JSON 设置样例解析、3,000 MB 配额预算与磁盘统计算术检查。两份 Mermaid 源码经本地 11.16.1 解析并在无头浏览器中渲染，已目视核对图形与文字；SVG 通过 diagram-generator 脚本写入交付目录。CLI 渲染器的网络安装曾因 ECONNRESET 失败，随后使用仓库现有 Mermaid 完成解析和渲染，不依赖新增全局工具。

可编辑源和 SVG 位于 `docs/diagrams/linux-vm/`；主方案已链接。最后检查本轮 Git 差异和要求覆盖表；未运行的生产验收始终保留“未验收”状态，不将文档检查写作生产测试通过。
