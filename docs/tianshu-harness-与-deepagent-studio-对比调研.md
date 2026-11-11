# Tianshu-harness 与 DeepAgent Studio 对比调研

> 调研快照：2026-11-11。本文是持续维护的源码调研基线，不是对上游产品宣传的复述。
> 源码版本：DeepAgent Studio `2fb5b9e7`；Tianshu-harness `2d0b22e0`，`package.json` 版本 `3.37.4`。DeepAgent 工作区在调研时存在其他未提交改动；本文引用的核心比较文件未显示在当时的改动清单中。
> 范围：本机 Agent 运行时、任务状态、提示词与上下文、压缩与记忆、工具和审批、代码理解、子代理、验证和开发者交互。
> 明确排除：远程连接能力、无限画布能力，以及星域/星辰的命名和叙事。只讨论这些命名背后的通用机制。

## 结论

两套系统的核心取向不同。DeepAgent Studio 已有更明确的 Rust 运行时边界和持久化执行骨架：`AgentKernel`、统一 `RunStore` / `run_events`、明确的运行阶段与终态、输入租约、审批、取消、沙箱后端和工具注册。Tianshu-harness 的显著长处在于“围绕模型每一轮工作建立认知控制”：它将用户目标整理为可推进的任务契约，以认知帧和分阶段 hook 观察运行状态，并把提示词增量、压缩回忆和知识准入放在同一条工作链中。

DeepAgent Studio 不缺少若干基础零件：已经有稳定/动态提示词边界、结构化压缩摘要、生命周期 Hook、构建验证计划、对抗式验证、代码图和 DAG 子代理。当前主要差距是这些能力还没有被统一表达为一个持续更新、可回放、能驱动交付判定的任务状态；与之相关的运行时认知反馈、压缩边界召回和缓存命中反馈也没有形成天枢那种紧密闭环。

本轮已先调整 DeepAgent Studio 的运行时主提示词，补上轻量任务契约、事实与推断区分、长任务恢复信息以及重复失败后重审设计边界的行为要求。提示词能改善模型行为，但不等同于持久化任务对象、确定性门禁或可审计的运行时状态；后续结构性借鉴仍需落在现有内核上。

## 一对一能力映射

| 编号与能力 | Tianshu-harness | DeepAgent Studio 当前对应 | 对比判断与借鉴方向 |
|---|---|---|---|
| 1. 任务契约 | 输入边界抽取 `objective / scope / constraints / successCriteria / status`，跟随后续输入更新，并投影到提示词。 | 有压缩用 `TaskSummary`（目标、已完成、待办、失败、决策）和基于实际改动触发的 `VerificationPlan`；未发现与用户输入同生命周期、可版本化的目标/范围/验收契约。 | **部分对应，契约层有差距。** 将任务契约作为现有 run 状态的投影或版本化事件，不能只靠模型每轮重新理解用户长消息。简单问答不应强行创建完整契约。 |
| 2. 事实帧与运行态势 | 每轮构造带质量标记和指纹的 `CognitiveFrame`，供控制器消费；运行时还计算压力和稳定性信号。 | `RunPhase`、`RuntimeEvent` 和持久化事件可重建执行过程；`InputLeaseRegistry` 管理同一 session 的输入排队。未发现对应的统一事实帧/态势摘要。 | **底层事实较强，面向认知的投影较弱。** 从已有事件和运行状态派生只读帧，标明来源、时间和可信度；不创建第二套事件账本。 |
| 3. 运行时 Hook 与 advisory | 五阶段 `preTurn / afterPerception / postTool / postTurn / postSession`，根据依赖和条件装配；提醒型 advisory 与工具安全硬门禁区分。 | 已有生产 `HookRegistry`，覆盖工具、权限、任务、子代理、压缩和会话等事件；它主要是可配置的生命周期动作/策略扩展，不等同于根据认知状态运行的 CVM hook 管线。 | **有 Hook 基础设施，运行时监督模型不同。** 可借鉴“提醒与硬门禁分型、按条件装配、单项预算/失败策略可观察”；沿用现有 hook 和 runtime seam，不再建一套注册表。 |
| 4. 完成交付与验证 | 通过任务契约、证据状态和 delivery gate 判断能否声称完成；另有重复循环和策略切换机制。 | 自动发现项目构建验证计划，基于真实文件改动触发；运行时执行验证。另有对抗式验证器，但该 verifier 被明确设计为 advisory，不会单独硬失败运行。 | **DeepAgent 的自动构建验证是强项；用户验收覆盖仍可加强。** 将用户明确的成功条件映射到现有验证计划和证据事件，并区分“构建通过”与“目标全部满足”。不要把 LLM 审查意见伪装成确定性事实。 |
| 5. 提示词与缓存 | `PromptEngine` 组织冻结基座、动态区和可配置的 `appendixDelta`；目标、任务进展和计划轨迹等状态可按变化增量更新。 | `system_prompt_base()` 提供稳定静态前缀，`system_context` 在动态边界后装配环境、权限和上下文；已有稳定前缀测试。没有发现同等的跨轮 appendix 序号/增量状态更新机制。 | **两边都重视稳定前缀，天枢的动态状态增量更细。** 在 DeepSeek exact-prefix 缓存配置上评估差异追加、状态快照和恢复；先以真实缓存 usage 做 A/B，避免只为“有 delta”而加协议复杂度。 |
| 6. 压缩与历史回忆 | 在压缩边界归档原始片段，并在后续提供显式回忆标记/读取路径；压缩受上下文压力和缓存收益共同影响。 | `TaskSummary` 是结构化摘要；应用侧压缩会保留修改文件、失败检查、已调用技能，并附带 session transcript 指针。压缩政策主要按 token/窗口预算决策；未发现同样明确的分段归档索引和 CacheAdvisor。 | **摘要和原始 transcript 保留已有，边界级召回及缓存反馈较弱。** 研究把归档 span 与引用标记接入既有 transcript/artifact/persistence 能力，并给压缩决策增加可解释的 cache 与上下文压力指标。 |
| 7. 跨会话知识准入 | `essence-gate` 检查可迁移性、重复、矛盾与失败经验；结构不合格或模型侧路失败时不写入，准入项要求有 `transferableTo`。 | 已有 session/project/global memory 和 knowledge；会话捕获可先进入草稿待确认，并有确定性、可逆的重复项合并。未发现所有自动捕获和直接写入入口都经过统一的语义准入/矛盾替代门。 | **知识治理部分对应。** 仅针对跨会话/项目知识研究准入：可迁移范围、证据来源、重复检测、矛盾替代；保留现有草稿确认和可逆合并，不要把会话内状态更新也阻塞在知识审查上。 |
| 8. 子代理与编排 | `Coordinator / team / council / starflow` 提供多种面向目标的编排层和 worker profile；这里不评价星域名称。 | `plan_execute` 由模型生成 DAG，经 `DagScheduler` 和同一套 subagent runner 执行，使用独立 Git worktree；另有单任务 `task` 路径。 | **DeepAgent 已有结构化、隔离的并行执行；天枢编排选项更丰富。** 先以不同任务类型评估 DAG 的分解、结果合并和复核质量；只有验证出缺口时再引入动态协调或多轮评议。 |
| 9. 代码库理解 | 有 Meridian 项目图、混合检索、LSP 诊断回流和面向代码关系的检索/分析能力。 | 有 `deepagent-codegraph`、项目地图/仓库工具和构建验证链。 | **存在直接对应，需用任务集测效果，不能按目录数判断胜负。** 对大型 Rust/TS 仓库比较符号定位、调用关系、影响范围和诊断闭环，再决定是否补混合检索或 LSP 集成。 |
| 10. 工具、权限、MCP | 工具注册、风险/权限管线、MCP 管理和工具清单快照等机制共同控制执行。 | 有统一 `ToolRegistry`、类型化 `ApprovalRequest`、权限模型、`SandboxBackend` seam；MCP catalog 以快照/版本管理，避免执行期间工具列表漂移。 | **这是 DeepAgent 的主要强项之一：安全边界显式且处于运行时核心。** 借鉴天枢的清单快照和审批指纹时，应核对现有 MCP catalog 与审批数据能否直接承载，避免重复审批路径。 |
| 11. 桌面与终端工作流 | 本仓可检查到 CLI/TUI；TUI 提供面向运行过程的终端控制与诊断。天枢桌面实现不在该源码仓，本表不评价其闭源实现。 | 仓内有 Tauri 桌面端和 CLI；CLI 通过 AppCore 接入运行能力；本文不比较远程连接和无限画布。 | **源码可见性不同，不能据此比较完整桌面产品。** 可借鉴天枢 cockpit 式运行态信息密度，但按 DeepAgent 的现有桌面设计系统实现。 |

## 差距与强项

### DeepAgent Studio 相比 Tianshu-harness 更强的地方

- **明确的 run 级生命周期契约。** `AgentKernel` 将 run 生命周期、`RunPhase` 和 `TerminalKind` 明确化；运行事件由持久化 sink 写入 `RunStore`，事件序号、游标、重连和终态投影有专项测试。Tianshu 也持久化 CLI transcript，并为 sidecar session 提供事件日志/续读；本项优势限于 DeepAgent 在已检查代码中把 run 终态和持久化事件作为显式类型化契约。
- **控制面与安全边界清晰。** 工具注册、审批请求、取消树、输入租约和沙箱后端在 Rust 类型/trait 边界表达，便于 CLI、桌面和后续协议层共用。Tianshu 也有权限、审批与沙箱，不应把它概括成无安全模型；DeepAgent 的优势是核心运行契约更集中、类型化。
- **仓内桌面承载。** DeepAgent Studio 源码包含 Tauri 桌面端和 CLI，并把 CLI 运行能力接到 AppCore/Runtime。Tianshu 桌面实现不在已检查的源码仓，不能据此判断其完整桌面产品强弱；此处不纳入远程连接和无限画布比较。
- **执行验证已经进入运行时。** 自动构建检查基于项目构建文件发现，并由真实文件改动触发；对抗式目标核验也已有实现。下一步是让用户验收标准成为同一证据链中的输入，而非另造第二个验证系统。
- **DAG 子代理隔离。** `plan_execute` 走共享 subagent runner 和 scheduler，并以 Git worktree 隔离并行节点，已有一个可继续演进的基础编排方案。

### DeepAgent Studio 相比 Tianshu-harness 的主要差距

- **缺少持续的任务契约对象。** 现在有摘要和验证计划，但目标、范围/排除项、约束、成功标准、阶段、证据没有成为一个随输入更新、压缩后恢复、最终交付消费的统一对象。
- **缺少认知监督投影。** 运行事件足以做事实来源，但目前没有类似 `CognitiveFrame` 的单一视图把证据质量、计划进展、重复失败、验证债等整合给控制器。现有 Hook 解决可扩展动作和事件策略，不自动等价于认知状态机。
- **提示词动态状态增量较粗。** 静态前缀和动态边界已经存在，前缀稳定性有测试；逐轮变化的目标/进度/计划状态还没有被建模为可恢复的 appendix delta。
- **压缩后可寻回性与缓存决策可观测性不足。** 有结构化摘要与 transcript 指针，但缺少明确的压缩 span → 归档 artifact → 后续精确召回的统一协议；压缩策略没有把缓存命中价值与上下文压力放入同一决策记录。
- **长期知识缺少统一的语义准入。** DeepAgent 已有会话捕获草稿确认、跨项目/全局分层和确定性可逆重复合并；差距是尚未看到统一门控所有自动捕获与直接写入的语义准入契约，尤其是可迁移范围、来源、矛盾替代和失败经验收录。
- **认知韧性机制需要补齐评估。** 对重复调用、重复失败、目标漂移和停滞的检测，现有 loop/验证/事件能力可以提供素材，但尚未看到统一、可解释、可测试的反馈面。

## 本轮提示词调整

修改位置：[system_prompt.rs](../crates/deepagent-app-core/src/system_prompt.rs)。新增要求聚焦四点：

1. 对实质性任务保留精简契约：目标、范围和排除项、约束、验收标准、基于证据的进展；简单问题继续直接处理。
2. 明确区分观察事实、推断和未决问题，完成声明必须由源码、工具结果或测试结果支撑。
3. 压缩/恢复上下文时保留目标、约束、决策、已知失败和未验证事项；必要时重新读取事实源。
4. 同一路线反复失败时重新检查假设或设计边界，不靠增加临时特判堆叠解决。

这一改动只加强模型的工作约束，不宣称实现了可持久化 `TaskContract`、运行时 `CognitiveFrame` 或机械化 DeliveryGate。静态提示词字节变化会让 DeepSeek 前缀缓存产生一次冷启动；随后应以真实 cache usage 观察长期影响。

## 建议的长期调研顺序

| 阶段 | 研究与实现方向 | 验收重点 |
|---|---|---|
| P0（已完成本轮） | 补齐主提示词的范围/验收、事实/推断、长任务恢复和重复失败重评规则。 | 单元测试保护新增提示词契约；静态前缀组装与稳定性测试通过。 |
| P1 | 设计版本化 `RunTaskContract`，消费用户明确的目标/约束/验收；其变更、进度和证据投影到现有 run 事件/状态。 | 会话续跑、追加用户指令、压缩恢复、用户改变范围、失败/取消等用例中不丢失旧决策或错误沿用旧验收。 |
| P1 | 把 `VerificationPlan` 的构建结果与用户验收标准关联，形成可解释的交付证据摘要。 | “构建通过但用户目标未覆盖”不能误判完成；环境受阻与测试失败可区分；不重复运行无关全量检查。 |
| P2 | 从 `RuntimeEvent`、验证结果和任务契约派生只读 CognitiveFrame；把 advisory 与确定性硬门禁在现有 Runtime/Hook 边界中区分。 | 每个信号可追到事件来源；hook 失败预算、条件装配、取消和权限行为有测试；不增加第二个事件/审批/工具系统。 |
| P2 | 对 DeepSeek provider 试验状态 appendix delta 和 prompt snapshot 恢复，并将缓存顾问决策及原因接入观测。 | 同一任务轮间前缀稳定；中断/续跑一致；比较缓存命中、输入 token、压缩时机和任务质量，不用上游自报命中率替代本地测量。 |
| P3 | 为压缩区间增加可回溯的归档标记与精确召回入口；复用 transcript/artifact/persistence。 | 任务标准、失败证据、设计决策和被压缩原文可定位；归档失败不破坏原始历史；召回行为可审计。 |
| P3 | 研究 project knowledge 的语义准入、去重、矛盾替代和来源记录。 | 只门控长期知识，不阻塞短期会话记忆；解析/模型失败行为明确，能够解释准入或拒绝原因。 |
| P4 | 对并行编排、代码检索和停滞恢复建立同一任务集的基准测试。 | 同模型、同工具权限和可比环境；比较成功率、人工接管次数、修复轮数、token/cache 成本、错误重复率、恢复率和权限违规。 |

## 证据索引与边界

| 主题 | Tianshu-harness 源码 | DeepAgent Studio 源码 |
|---|---|---|
| 任务契约、事实帧、运行 Hook | [`task-contract.ts`](../借鉴/Tianshu-harness/src/context/task-contract.ts)、[`human-input-boundary.ts`](../借鉴/Tianshu-harness/src/agent/human-input-boundary.ts)、[`cognitive-frame.ts`](../借鉴/Tianshu-harness/src/agent/cognitive-frame.ts)、[`runtime-hooks.ts`](../借鉴/Tianshu-harness/src/agent/runtime-hooks.ts) | [`kernel.rs`](../crates/deepagent-runtime/src/kernel.rs)、[`events.rs`](../crates/deepagent-runtime/src/events.rs)、[`hook_assembly.rs`](../crates/deepagent-app-core/src/hook_assembly.rs) |
| 提示词、压缩与缓存 | [`engine.ts`](../借鉴/Tianshu-harness/src/prompt/engine.ts)、[`volatile.ts`](../借鉴/Tianshu-harness/src/prompt/volatile.ts)、[`boundary-archive.ts`](../借鉴/Tianshu-harness/src/compact/boundary-archive.ts)、[`advisor.ts`](../借鉴/Tianshu-harness/src/cache/advisor.ts) | [`system_prompt.rs`](../crates/deepagent-app-core/src/system_prompt.rs)、[`system_context.rs`](../crates/deepagent-app-core/src/system_context.rs)、[`assembler.rs`](../crates/deepagent-context/src/assembler.rs)、[`compaction.rs`](../crates/deepagent-context/src/compaction.rs)、[`context_runtime.rs`](../crates/deepagent-app-core/src/context_runtime.rs) |
| 记忆、交付与验证 | [`essence-gate.ts`](../借鉴/Tianshu-harness/src/memory/essence-gate.ts)、[`delivery-gate.ts`](../借鉴/Tianshu-harness/src/agent/delivery-gate.ts) | [`memory_service.rs`](../crates/deepagent-app-core/src/memory_service.rs)、[`memory_consolidation.rs`](../crates/deepagent-app-core/src/memory_consolidation.rs)、[`knowledge_service.rs`](../crates/deepagent-app-core/src/knowledge_service.rs)、[`completion_plan.rs`](../crates/deepagent-app-core/src/completion_plan.rs)、[`verification_panel.rs`](../crates/deepagent-app-core/src/verification_panel.rs) |
| 运行存储、安全与并行代理 | [`tool-pipeline.ts`](../借鉴/Tianshu-harness/src/agent/tool-pipeline.ts)、[`coordinator.ts`](../借鉴/Tianshu-harness/src/agent/coordinator.ts)、[`team-orchestrator.ts`](../借鉴/Tianshu-harness/src/agent/team-orchestrator.ts)、[`session-persistence.ts`](../借鉴/Tianshu-harness/src/server/session-persistence.ts) | [`run_store.rs`](../crates/deepagent-persistence/src/run_store.rs)、[`registry.rs`](../crates/deepagent-tools/src/registry.rs)、[`sandbox_backend.rs`](../crates/deepagent-app-core/src/sandbox_backend.rs)、[`dag_orchestration.rs`](../crates/deepagent-app-core/src/dag_orchestration.rs)、[`subagent_runner.rs`](../crates/deepagent-app-core/src/subagent_runner.rs) |

本文的“当前未发现”只表示在上述生产源码边界和该版本快照中没有找到等价的统一机制，不表示仓库所有实验分支或历史计划都不存在相关概念。Tianshu 导读中官方自述的缓存命中率、hook 数量和测试用例规模不当作独立验证结果；本调研只把源码结构和可运行测试视为实现证据。每次复查上游版本时，应更新版本快照、重新核对生产接线、测试覆盖和实测效果，再调整建议优先级。
