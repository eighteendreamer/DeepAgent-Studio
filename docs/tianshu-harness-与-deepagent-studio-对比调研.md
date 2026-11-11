# Tianshu-harness 与 DeepAgent Studio 对比调研

> 状态：第一轮源码调研基线，2026-11-11 二次复核；不是性能评测，也不是升级功能已完成的声明。
> 版本：Tianshu-harness `2d0b22e0` / package `3.37.4`；DeepAgent 基线 `2fb5b9e7`，首轮提示词提交 `7339cd71`。本次复核直接读取当前工作区；模型文件引用只评价已有 provider seam，不评价其他未提交的 Files API 改动。
> 排除：远程连接、远程访问/协作/daemon 部署、无限画布；不比较星辰命名、角色包装和叙事。stdio/本机 SDK 作为本地入口仍在范围内。
> 阅读顺序：先看结论和优先级，再按编号看一对一映射；重要差异有独立源码复核；最后是可续接的研究台账与实际验证记录。

## 1. 先给结论

**最值得借鉴的是任务证据链和状态投影，而不是另造一套运行时。** 天枢把目标/约束、任务进展、证据质量、失败轨迹和提醒组织成每轮可消费的状态；DeepAgent 已经拥有承载这些投影所需的 `AgentKernel`、`RunStore`、`run_events`、`RunPhase`、`TerminalKind`、输入租约、审批、取消树、工具/Hook 注册和沙箱边界。升级应在这些已有事实源上增加投影和策略。

**DeepAgent 不是“少一个名字就少一项能力”。** 本系统已有：重复调用检测、失败升级提示、可选停滞分类、可选对抗式验证、构建验证与编辑后验证、持久化计划模式、三级技能加载、延迟工具发现、MCP 目录版本、结构化压缩、滚动 session memory、混合知识检索、文件 checkpoint、DAG/worktree 子代理、成本记账以及本地 SDK。上一版将部分能力概括为“缺失”或合并过粗，本版按机制修正。

**优先补的缺口：**

1. 将用户明确的范围/排除项、约束、验收项与证据关联，并在输入追加、压缩和恢复后保留。
2. 在现有运行事件上投影任务事实帧和证据状态，统一提醒的预算、去重、优先级及效果观测。
3. 研究 DeepSeek 的冻结历史与动态状态增量、缓存保护和压缩决策归因；不是只测试静态 system 前缀相等。
4. 区分 MCP “本轮目录一致性”和“变更后重新接受/审批”；前者已有，后者值得专门评估。
5. 补长期知识的来源、适用范围和矛盾替代；保留现有草稿确认、去重与可逆归档。
6. 将代码图与 LSP 诊断回流作为互补能力，不能用树解析调用图代替编译器语义诊断。

**DeepAgent 可保留并放大的长处：** 明确的 run 级阶段/终态与持久化控制契约，本地 stdio 协议＋Rust/TS SDK，原生代码图对 AI 工具和项目地图的复用，Windows 沙箱适配边界，以及同一执行链上的应用服务。这里的“长处”是源码可见的结构性基础，不是对天枢整体成功率、安全性或性能的排名。

## 2. 判断口径

每项都按“**源码证据 → 行为结论 → 本仓落点与验收**”组织：

- **已有对应**：不应新增第二套实现；可以研究效果或集成质量。
- **部分对应**：能力已有，但数据粒度、默认启用条件或闭环不同。
- **当前未见等价入口**：在本文给出的生产装配/调用范围内未找到；不等于证明整个仓库、历史分支或插件生态绝不存在。
- **不可直接比较**：闭源、需真实平台或尚未进行同条件评测。

源码中的接口、注释、测试用例存在、测试实际通过、真实模型/平台表现是五种不同证据。本文不把类名、测试数量、文档计划或上游命中率宣传当作效果证明；带条件的机制必须保留启用条件。内部链接面向当前本地仓库；`借鉴/` 是只读参考，不复制业务代码。

## 3. 一对一能力映射

编号是长期台账的稳定索引；“优先级”指研究顺序，尚未实现的项目不因写进表格就变成已交付功能。

### 3.1 目标、执行与交付

| 编号 / 能力 | 天枢：源码与实际机制 | DeepAgent：对应源码与状态 | 判断 / 借鉴落点 |
|---|---|---|---|
| A01 任务目标/范围/验收 | [task-contract.ts](../借鉴/Tianshu-harness/src/context/task-contract.ts) 与 [human-input-boundary.ts](../借鉴/Tianshu-harness/src/agent/human-input-boundary.ts)：维护目标、提到的文件、约束和状态，后续输入可合并。默认成功条件仍是通用文本，不是完整语义需求提取。 | [TaskSummary](../crates/deepagent-context/src/compaction.rs) 有目标/待办/失败/决策；[CompletionPolicy](../crates/deepagent-runtime/src/completion.rs) 从已执行工具效果验文件变化；未见统一输入生命周期的任务验收对象。 | **部分对应，P1。** 本地设计：从用户明确要求提取可追踪的验收项，在既有事件中保存版本与来源；不照搬文件正则和默认成功标准。 |
| A02 每轮只读事实帧 | [cognitive-frame.ts](../借鉴/Tianshu-harness/src/agent/cognitive-frame.ts)、[loop.ts](../借鉴/Tianshu-harness/src/agent/loop.ts)：整合验证债、失败、阶段/计划、用户介入、进度，并带质量/指纹。 | [events.rs](../crates/deepagent-runtime/src/events.rs)、[kernel.rs](../crates/deepagent-runtime/src/kernel.rs) 有执行事实、阶段与终态，尚未见同样统一的认知帧。 | **部分对应，P1/P2。** 从现有事件投影，不另建认知事件 store。 |
| A03 Hook 与提醒调度 | [runtime-hooks.ts](../借鉴/Tianshu-harness/src/agent/runtime-hooks.ts)、[create-runtime-hooks.ts](../借鉴/Tianshu-harness/src/agent/create-runtime-hooks.ts)、[advisory-bus.ts](../借鉴/Tianshu-harness/src/agent/advisory-bus.ts)：五阶段、条件装配、提醒去重/优先级/TTL，支持采纳观察。 | [hook_assembly.rs](../crates/deepagent-app-core/src/hook_assembly.rs) 生产注册用户/项目/插件 Hook；[model_agent.rs](../crates/deepagent-runtime/src/model_agent.rs) 注入 todo、失败/停滞提醒。 | **部分对应，P2。** 缺口在跨提醒来源的统一预算/效果核销，不是缺 Hook；复用现有 registry 和 reminder。 |
| A04 验证与交付证据 | [delivery-gate-v2.ts](../借鉴/Tianshu-harness/src/agent/delivery-gate-v2.ts) 评估归属/验证；当前 [deliver-task.ts](../借鉴/Tianshu-harness/src/agent/deliver-task.ts) 将验证状态披露与安全提交分开，不要求每次提交先通过测试/审查。 | [completion_plan.rs](../crates/deepagent-app-core/src/completion_plan.rs)＋[run_assembler.rs](../crates/deepagent-app-core/src/run_assembler.rs) 接构建验证；[verification_decorator.rs](../crates/deepagent-app-core/src/verification_decorator.rs) 支持默认提醒/Strict；[loop_engine.rs](../crates/deepagent-runtime/src/loop_engine.rs) 耗尽修复策略可收尾。 | **已有多层验证，P1 改进证据关联。** 构建通过、提交成功、用户验收满足是三件事；不能称任一方“总能阻断未验证交付”。 |
| A05 重复调用/失败/停滞 | [trace-store.ts](../借鉴/Tianshu-harness/src/agent/trace-store.ts)、[execution-guidance.ts](../借鉴/Tianshu-harness/src/agent/execution-guidance.ts)、[strategy-shift.ts](../借鉴/Tianshu-harness/src/agent/strategy-shift.ts) 组合指纹和轨迹反馈。 | [model_agent.rs](../crates/deepagent-runtime/src/model_agent.rs) 已有 doom-loop、失败窗口/策略切换提示；[stall_detector.rs](../crates/deepagent-runtime/src/stall_detector.rs) 有预算化语义停滞判断，装配由开关决定。 | **已有对应，P2 测效果。** 补跨信号一致性、误报/采纳率；不再提议新建防循环系统。 |
| A06 计划生命周期/波间验收 | [plan-approval.ts](../借鉴/Tianshu-harness/src/plan/plan-approval.ts)、[plan-store.ts](../借鉴/Tianshu-harness/src/plan/plan-store.ts)、[wave-gate.ts](../借鉴/Tianshu-harness/src/agent/wave-gate.ts)：计划文档批准/漂移提醒/关闭、可配置波间验证。 | [plan_mode.rs](../crates/deepagent-builtins/src/plan_mode.rs) 从 session 事件恢复模式并门控写/执行；[dag_orchestration.rs](../crates/deepagent-app-core/src/dag_orchestration.rs) 提供 DAG。 | **部分对应，P2。** 计划模式不是批准后的文档任务契约；先补计划版本、步骤与证据引用，再考虑波间策略。 |
| A07 长期目标续跑 | [goal-tracker.ts](../借鉴/Tianshu-harness/src/agent/goal-tracker.ts)、[goal-continuation.ts](../借鉴/Tianshu-harness/src/agent/goal-continuation.ts) 有目标状态/迭代预算/判定与上下文接力条件；judge 次数也有上限。 | [AgentKernel](../crates/deepagent-runtime/src/kernel.rs)＋[input.rs](../crates/deepagent-runtime/src/input.rs) 支持 run、取消、steer/排队；未在核对范围找到同等的跨会话 goal 控制器。 | **部分对应，P3。** 若需要长期自治，在现有 run/continuation 控制之上投影 goal；不把“普通继续会话”当成目标完成证明。 |
| A08 子代理工单/编排 | [work-order.ts](../借鉴/Tianshu-harness/src/agent/work-order.ts)、[coordinator.ts](../借鉴/Tianshu-harness/src/agent/coordinator.ts)、[team-orchestrator.ts](../借鉴/Tianshu-harness/src/agent/team-orchestrator.ts) 有工单、工具范围、预算、模型选择及团队调度。 | [subagent_runner.rs](../crates/deepagent-app-core/src/subagent_runner.rs)、[scheduler.rs](../crates/deepagent-subagents/src/scheduler.rs) 共用子代理链，DAG 节点 worktree 隔离，依赖摘要/输出基线传递。 | **已有核心对应，P3。** 借鉴工单中的验收、证据/不确定性、范围与预算，而非复制多种编排名称；worktree 输出不会自动变成主工作区修改。 |

### 3.2 提示词、压缩与记忆

| 编号 / 能力 | 天枢：源码与实际机制 | DeepAgent：对应源码与状态 | 判断 / 借鉴落点 |
|---|---|---|---|
| B01 稳定前缀 | [engine.ts](../借鉴/Tianshu-harness/src/prompt/engine.ts)、[create-agent-config.ts](../借鉴/Tianshu-harness/src/agent/create-agent-config.ts)：冻结 system/用户上下文与历史锚点。 | [system_prompt.rs](../crates/deepagent-app-core/src/system_prompt.rs)＋[system_context.rs](../crates/deepagent-app-core/src/system_context.rs) 保持静态基座/动态边界，生产走 [ContextAssembler](../crates/deepagent-context/src/assembler.rs)。 | **已有对应，P1 检查完整请求。** 静态前缀相等不证明整段历史缓存最佳；按 wire request 验证。 |
| B02 动态状态增量 | [engine.ts](../借鉴/Tianshu-harness/src/prompt/engine.ts) 的 appendix seq/full/delta；[volatile.ts](../借鉴/Tianshu-harness/src/prompt/volatile.ts) 将目标/进展等拆分；历史重写后 reset baseline。生产配置默认开，可显式关闭。 | 动态上下文和 [skill_catalog_reminder.rs](../crates/deepagent-app-core/src/skill_catalog_reminder.rs) 存在；后者名称含 next_delta，但实现每次返回完整当前可见目录，服务无状态续跑。 | **部分对应，P2。** 增量必须有可恢复快照；不能为省 token 直接改成 send-once，造成续跑丢目录。 |
| B03 缓存感知压缩 | [advisor.ts](../借鉴/Tianshu-harness/src/cache/advisor.ts) 综合近期命中率、窗口压力/温度，记录 delay/allow 原因；高危层不延迟。 | [policy.rs](../crates/deepagent-context/src/policy.rs)、[meter.rs](../crates/deepagent-context/src/meter.rs) 处理能力窗口、输出余量/工具及压力计量；未发现同等的命中收益反馈决策器。 | **部分对应，P2。** 可借鉴决策输入与归因；阈值必须在本地任务集验证，不能直接搬数值。 |
| B04 压缩原文召回 | [boundary-archive.ts](../借鉴/Tianshu-harness/src/compact/boundary-archive.ts)、[compaction-controller.ts](../借鉴/Tianshu-harness/src/agent/compaction-controller.ts)：截断前归档工具原文、保留引用；归档失败保留原文。 | [context_runtime.rs](../crates/deepagent-app-core/src/context_runtime.rs) 保留工作集/摘要并带 transcript 指针；[session_memory.rs](../crates/deepagent-app-core/src/session_memory.rs) 有安全摘要游标；大工具结果另有 durable artifact。 | **部分对应，P2/P3。** 要补的是压缩 span 到原文的精确定位/召回协议，不是重建 transcript 或 artifact store。 |
| B05 会话内事实/claims | [claim-store.ts](../借鉴/Tianshu-harness/src/context/claim-store.ts) 有提议、状态/证据、消费与 checkpoint；不要因存在 claims 类就假设全部主提示渲染路径在用。 | 滚动 [session_memory.rs](../crates/deepagent-app-core/src/session_memory.rs) 记录会话摘要，事件身份/双游标限制错误切断；[TaskSummary](../crates/deepagent-context/src/compaction.rs) 保存失败/决策。 | **部分对应，P2。** 引入证据质量字段/引用的价值较高；不必把所有自然语言记忆改造成 claim 数据库。 |
| B06 长期知识准入 | [essence-gate.ts](../借鉴/Tianshu-harness/src/memory/essence-gate.ts) 要求适用范围、去重/矛盾替代，侧路失败不入库。 | [knowledge_service.rs](../crates/deepagent-app-core/src/knowledge_service.rs) 有草稿/接受、捕获/直接保存；[memory_consolidation.rs](../crates/deepagent-app-core/src/memory_consolidation.rs) 可逆去重；[memory_service.rs](../crates/deepagent-app-core/src/memory_service.rs) 复用同一知识库。 | **部分对应，P2。** 补长期条目的来源、适用范围、矛盾与过期；已有草稿确认和可逆合并应保留。 |
| B07 混合知识检索 | [knowledge-index.ts](../借鉴/Tianshu-harness/src/memory/knowledge-index.ts) 使用 [hybrid-search.ts](../借鉴/Tianshu-harness/src/search/hybrid-search.ts) 的 RRF。 | [contextual_retrieval.rs](../crates/deepagent-memory/src/contextual_retrieval.rs) 已有 dense＋BM25/RRF；生产知识服务使用 [HashingEmbedder](../crates/deepagent-memory/src/embedding.rs)。 | **已有算法对应。** 不是缺混合检索；真正要测的是 embedding 质量、中文/概念检索与来源过滤。不能把 hashing 向量直接等同于语义模型效果。 |

### 3.3 工具、安全与恢复

| 编号 / 能力 | 天枢：源码与实际机制 | DeepAgent：对应源码与状态 | 判断 / 借鉴落点 |
|---|---|---|---|
| C01 工具可见性/延迟发现 | [tool-tiers.ts](../借鉴/Tianshu-harness/src/agent/tool-tiers.ts) 区分 core/extended/full，工具 preset 影响装配。 | [tool_search.rs](../crates/deepagent-builtins/src/tool_search.rs) 有 Disabled/Enabled/Auto、deferred snapshot/discovered-set；[tool_runtime.rs](../crates/deepagent-app-core/src/tool_runtime.rs) 统一装配。 | **已有对应，本系统有通用懒发现。** 借鉴可见性预算/使用指标即可；不硬编码“必须 ≤26 件工具”，当前天枢列表也受条件影响。 |
| C02 MCP 目录一致性 | [McpManager](../借鉴/Tianshu-harness/src/mcp/manager.ts) 连接/发现并挂载工具；与清单门共同管理版本变动。 | [catalog.rs](../crates/deepagent-mcp/src/catalog.rs) 发布不可变 revision，刷新失败撤下工具；[run_capabilities.rs](../crates/deepagent-app-core/src/run_capabilities.rs) 保存本次实际能力 hash。 | **已有对应。** 目录一致性和 hash 证据不是审批授权；复用当前机制。 |
| C03 MCP 清单变更再接受 | [tool-inventory.ts](../借鉴/Tianshu-harness/src/mcp/tool-inventory.ts) 对描述/schema 做 hash/diff、批准显示版本后 arm，下次精确匹配才放行；[server-approval.ts](../借鉴/Tianshu-harness/src/mcp/server-approval.ts) 独立管连接配置。 | C02 的 revision、[adapter.rs](../crates/deepagent-mcp/src/adapter.rs) 的工具权限和统一审批已有；未见同等的“已接受清单→变更→用户再确认”持久化流程。 | **不同安全目的，P1。** 优先评估重连/热更新的 rug-pull 风险与现有审批数据映射；不能把 hash 相等测试当成人工授权测试。 |
| C04 插件扩展与隔离 | [manifest.ts](../借鉴/Tianshu-harness/src/plugins/manifest.ts) 明示 permissions 仅为声明，进程内 JS 可用完整 Node 权限。 | [plugin_runtime.rs](../crates/deepagent-app-core/src/plugin_runtime.rs) 将 skills/hooks/MCP/commands 等投影到现有子系统；[plugin_security.rs](../crates/deepagent-app-core/src/plugin_security.rs) 是静态预检；[wasm.rs](../crates/deepagent-tools/src/wasm.rs) 是单独 feature 下的受限执行面。 | **结构不同。** 本系统的声明式装配与可选 WASM 边界值得保留；不能称所有插件都受 WASM 隔离，也不能将静态扫描当沙箱。 |
| C05 技能渐进加载 | [skill-loader.ts](../借鉴/Tianshu-harness/src/skills/skill-loader.ts) 提供目录/正文/资源分级，按相关性和预算组织 discovery。 | [loader.rs](../crates/deepagent-skills/src/loader.rs)、[skill_tool.rs](../crates/deepagent-builtins/src/skill_tool.rs) 已有发现、正文调用、按激活读取资源与模型禁用开关。 | **已有对应。** 可比较大目录排序、相关性、续跑与插件 skill 一致性；不再建第二套 loader。 |
| C06 命令沙箱/平台边界 | [sandbox-profile.ts](../借鉴/Tianshu-harness/src/tools/sandbox-profile.ts) 有 macOS/Linux/WSL 后端，原生 Windows 需依赖审批等补偿；旁路有独立策略。 | [sandbox_backend.rs](../crates/deepagent-app-core/src/sandbox_backend.rs) 显式区分 Direct/Sandboxie/WindowsSandbox，并提供能力查询。 | **平台各有所长。** Windows seam 是本系统基础优势，Linux/macOS 不应套用 Windows 结论；本轮未做系统级隔离实测。 |
| C07 工作区信任/配置来源 | [manager.ts](../借鉴/Tianshu-harness/src/config/manager.ts)、[project-trust.ts](../借鉴/Tianshu-harness/src/config/project-trust.ts) 处理配置分层与不可信项目键。 | [trust_service.rs](../crates/deepagent-app-core/src/trust_service.rs)、[hook_assembly.rs](../crates/deepagent-app-core/src/hook_assembly.rs) 有来源/项目 hook 授权；未信任项目 bash 升级审批由环境开关启用，默认关闭。 | **已有但条件不同，P1 审计。** 禁止仅因有 TrustService 就宣称所有入口默认 fail-closed。 |
| C08 Checkpoint/撤销 | [checkpoint.ts](../借鉴/Tianshu-harness/src/agent/checkpoint.ts)、[file-history.ts](../借鉴/Tianshu-harness/src/agent/file-history.ts) 有回滚/文件历史与跨会话 ownership guard。 | [checkpoint.rs](../crates/deepagent-runtime/src/checkpoint.rs)＋[restore.rs](../crates/deepagent-runtime/src/checkpoint/restore.rs) 捕获写前状态，恢复可补偿并关联历史锚点。 | **已有对应。** 不宣称任一方可无条件回滚 shell 外部副作用；本系统恢复也不是跨进程 crash-atomic 事务。 |
| C09 大工具结果/产物 | [artifact/store.ts](../借鉴/Tianshu-harness/src/artifact/store.ts) 与工具截断/读取引用。 | [tool_budget.rs](../crates/deepagent-runtime/src/tool_budget.rs) 截断并持久化完整结果；[artifact_store.rs](../crates/deepagent-persistence/src/artifact_store.rs) 管产物。 | **已有对应。** 统一边界召回可沿用；“工具返回一个路径”不证明 host 已创建/授权该产物。 |

### 3.4 本地入口、代码理解与应用服务

| 编号 / 能力 | 天枢：源码与实际机制 | DeepAgent：对应源码与状态 | 判断 / 借鉴落点 |
|---|---|---|---|
| D01 代码图/影响范围 | [repo-map.ts](../借鉴/Tianshu-harness/src/tools/repo-map.ts)＋[meridian-db.ts](../借鉴/Tianshu-harness/src/repo/meridian-db.ts) 提供代码图/关系。 | [deepagent-codegraph](../crates/deepagent-codegraph/src/lib.rs) 用 tree-sitter/SQLite/FTS5，同一提取供 AI codegraph 工具和项目地图；[tool_runtime.rs](../crates/deepagent-app-core/src/tool_runtime.rs) 懒建索引。 | **已有核心对应。** 本系统原生索引复用应保留；图的语言精度/影响半径需任务集测量。 |
| D02 语义代码搜索与 LSP | [semantic-search.ts](../借鉴/Tianshu-harness/src/tools/semantic-search.ts) 接混合索引；[lsp/manager.ts](../借鉴/Tianshu-harness/src/lsp/manager.ts)＋[tool-pipeline.ts](../借鉴/Tianshu-harness/src/agent/tool-pipeline.ts) 在编辑后同步文件并拉诊断。 | D01 提供符号/调用图与 FTS，编辑后有构建/类型检查；核对的生产工具装配未见同等 LSP manager/诊断回流。B07 的知识检索不能代替代码检索。 | **部分对应，P2。** 优先验证增量诊断的延迟/准确性和中文概念找代码，再决定是否接 LSP/语义索引。 |
| D03 Provider 适配/选模 | [api/factory.ts](../借鉴/Tianshu-harness/src/api/factory.ts)、[provider-profile.ts](../借鉴/Tianshu-harness/src/api/provider-profile.ts) 有多协议/缓存 profile；[model/capability.ts](../借鉴/Tianshu-harness/src/model/capability.ts) 含任务路由评分。 | [model_runtime.rs](../crates/deepagent-app-core/src/model_runtime.rs)＋[models/capability.rs](../crates/deepagent-models/src/capability.rs) 有 DeepSeek 方向、角色选择、fallback 和能力窗口解析。 | **取向不同，P3。** 不是一概缺选模；可借鉴显式 provider 能力/缓存语义，不能把静态能力卡分数当实测质量，也不擅自改默认供应商。 |
| D04 Usage/成本/预算 | [token-ledger/store.ts](../借鉴/Tianshu-harness/src/token-ledger/store.ts) 增量消费 cache-log，汇总请求/cache/cost；缓存顾问单独决策。 | [call_usage.rs](../crates/deepagent-app-core/src/call_usage.rs)、[cost_service.rs](../crates/deepagent-app-core/src/cost_service.rs) 对物理调用与侧路计费，持久化成本/预算，后端计算供 UI。 | **已有对应。** 缺的是把成本/命中反馈接入策略效果分析，不是再写一套 token 账本；本轮不评价价格是否为最新报价。 |
| D05 本机机器协议/SDK | [headless.ts](../借鉴/Tianshu-harness/src/headless.ts)＋[stream-json.ts](../借鉴/Tianshu-harness/src/stream-json.ts) 有 headless/NDJSON；本项不评远程服务。 | [harness-protocol](../crates/deepagent-harness-protocol/src/lib.rs)、[app_server.rs](../apps/cli/src/app_server.rs)、[TS SDK](../packages/sdk/README.md) 和 [Rust SDK](../crates/deepagent-harness-sdk/src/lib.rs) 有本地 JSON-RPC、握手/回执校验、控制/续读。 | **本系统有明确机器协议基础。** 沿现有 RuntimeEvent 投影扩展契约，不能另建 SDK 专用执行/审批/持久化。 |
| D06 TUI/桌面运行观察 | [tui/engine/app.ts](../借鉴/Tianshu-harness/src/tui/engine/app.ts) 与 [cockpit](../借鉴/Tianshu-harness/src/tui/cockpit) 提供终端过程面板；天枢桌面实现不在本源码仓。 | [CLI TUI](../apps/cli/src/tui/app.rs) 和 [desktop/src](../apps/desktop/src) 均有源码。 | **可借鉴终端信息组织；完整桌面产品不可比较。** 只把本仓可见性当事实，不据此声称桌面 UX 胜出。 |
| D07 Office/本地产物 | [office-writer.ts](../借鉴/Tianshu-harness/src/tools/office-writer.ts)、[office-reader.ts](../借鉴/Tianshu-harness/src/tools/office-reader.ts) 与 office 插件覆盖文档工作。 | [office_service.rs](../crates/deepagent-app-core/src/office_service.rs) 有纯 Rust docx/xlsx 基础生成、读取及托管转换运行时。 | **已有对应。** 本系统无外部运行时的基础生成是可见优势；复杂版式、图表和真实 Office 兼容性未实测，不判高下。 |
| D08 本地移动开发调试 | 核对到 [cli-discover.ts](../借鉴/Tianshu-harness/src/tools/cli-discover.ts) 等通用 CLI 能力，未在本次工具/子系统清单找到与本系统等价的设备 DTO/快照服务；不排除技能调用 adb。 | [mobile_service.rs](../crates/deepagent-app-core/src/mobile_service.rs) 接 mobile-runtime、设备 registry/快照和 Android 后端。 | **本系统有专用应用集成基础。** 不把 crate/DTO 存在写成 Android/iOS 全平台实机可用；未进行设备验证。 |
| D09 定时任务/无人值守 | [schedule/tool.ts](../借鉴/Tianshu-harness/src/tools/schedule/tool.ts) 依赖活跃 scheduler 条件注册；[cron-wiring.ts](../借鉴/Tianshu-harness/src/server/cron-wiring.ts) 接同一运行时；本项只看本机调度语义。 | [cron_service.rs](../crates/deepagent-app-core/src/cron_service.rs) 从 occurrence 经正常 ChatService admission 启动，关联既有 run。 | **已有对应，天枢触发类型更丰富。** 可比触发/审核/恢复语义，不复制独立 scheduler 或无权限后台执行入口。 |
| D10 评测/回归 | [benchmark/runner.ts](../借鉴/Tianshu-harness/src/benchmark/runner.ts) 要求 live 模式配置真实 executor；题集与 dry-run 结果分开。 | [e2e_stabilization.rs](../crates/deepagent-runtime/tests/e2e_stabilization.rs)、[kernel_v2_e2e](../crates/deepagent-app-core/tests/kernel_v2_e2e.rs) 有脚本模型＋生产链路回归。 | **工程回归已有，质量排名未证实。** 增补同模型/同权限/同任务的真实基准，不能用离线绿测证明真实模型更强。 |

## 4. 必须保留的源码细节

### 4.1 任务契约值得借鉴，但提取算法不能照搬

证据：天枢 `extractTaskContract()` 位于 `context/task-contract.ts`；文件范围来自 `FILE_PATTERN`，约束靠必须/不要等标记分句，`defaultSuccessCriteria()` 只有“处理所请求行为”“相关验证完成或标为未验证”等通用项。输入边界负责创建/合并，`renderTaskAnchor()` 负责再次注入。它解决了“有可保留对象”，并未证明完整理解所有显式验收要求。

本地设计：在现有输入/事件边界保留用户原始要求的引用，再提取可解释条目；每项保留来源、版本、范围、证据与四态（已验证/未满足/受阻/未验证）。不能仅从动作关键词推导“必须创建文件”，也不能把自然语言任务合同作为权限授权。

验收：用户追加“不要碰 A”，改验收条件、压缩后继续、短句“继续”、取消后重开，都不能误丢约束或沿用失效的成功声明。应分别测试“准确保留用户要求”和“运行时执行事实”，不要让同一模型自行改标准后自行通过。

### 4.2 完成、验证、Git 提交必须分开解释

天枢当前 `agent/deliver-task.ts` 开头及执行分支明确：2026-10-25 后工具做 ownership/scope/Git 安全提交，验证和审查独立；`blockingReason` 被显示为“不阻断提交，交付报告须披露”。`DeliveryGateV2` 仍有价值，但不是该工具全局的质量 veto。`turn-end.ts` 的 v2 状态计算失败会降为 undefined，不能由类型名推出所有入口 fail-closed。

DeepAgent 也有三种不同语义：

- `VerificationDecorator`：默认 Enabled 只追加失败提醒；Strict 会把确定的 Failed 改成工具失败，TimedOut/Skipped 不如此处理。
- `VerificationPlan`：生产 `run_assembler.rs` 发现计划并接到 kernel。失败反馈可驱动自修；`verify_after_completion()` 的 GiveUp 路径仍接受收尾，不能称其为“测试永不通过就永不结束”。
- 对抗式 verifier：按开关装配，限定重入次数，错误 fail-open，是辅助而非不可绕过的目标完成证明。

结论：本系统的首要改进是让最终事实与措辞一致并覆盖用户标准，而非不分任务增加硬闸。本轮提示词明确“构建通过只证明该构建检查”“尽力结束不把失败变成成功”。运行时是否将未验证任务映射为特殊终态，是另一个需要协议兼容评审的本地设计，本轮未修改终态语义。

### 4.3 防循环已经存在，缺口是联合观测

DeepAgent `ModelAgent::maybe_inject_doom_loop_nudge()` 对重复工具参数签名一次提醒；处理工具结果的失败窗口会注入改变路线的建议；`maybe_stall_nudge()` 受分类器、置信度和单 run 预算约束；生产 `run_assembler.rs` 依据功能开关/环境配置接线。因此不能写成“本系统没有撞墙换策略”。

天枢的参考价值是把轨迹、验证债、提醒优先级/TTL 和采纳观察联系起来。提醒采纳率本身也只是代理指标：调用了期望工具不等于任务成功。下一步应测重复率、无进展轮数、误报率和最终质量，而不是增加更多同类提醒。

### 4.4 缓存优化必须对着完整 wire request 测

DeepAgent 的静态前缀、来源预算、模型请求计量、历史恢复和工具目录都有已有链路。天枢进一步冻结历史用户消息、对 appendix 做差异更新，并在压缩后重建完整 baseline。生产 `create-agent-config.ts` 默认启用 appendixDelta，环境变量可关闭。

借鉴时不能：

- 把天枢的命中率宣传当作本系统必然收益。
- 把完整技能目录改成一次性发送，破坏无状态续跑。
- 无条件推迟压缩：高窗口压力/服务端溢出应优先保可运行性。
- 为“缓存稳定”保留错误权限或过期能力。缓存一致性不能替代权限撤销。

已有 [cache_prefix.rs](../crates/deepagent-app-core/tests/kernel_v2_e2e/cache_prefix.rs) 包含生产请求前缀测试；后续 A/B 应记录完整请求首个差异、缓存 hit/miss、压缩原因、恢复质量和真实 token 成本。

### 4.5 MCP 快照与审批快照是两件事

DeepAgent `ToolCatalog` 在刷新后发布不可变 generation，run capability 记录实际 schema hash；这些是审计/一致性证据，文件也明确自身不是授权引擎。

天枢 `evaluateInventoryGate()` 比较“上次接受的清单”与新清单，描述/schema 变更触发待接受状态；批准的是显示的 hash，重拉不匹配继续拦。连接配置指纹审批在另一个 store。它也有明确例外：显式 open 模式可放行；坏 inventory 文件按首次处理。因此借鉴重点应是变更再接受与 TOCTOU 测试，不是照搬其故障策略或额外审批中心。

验收：配置身份不变但描述/schema 改动、用户批准后再改、无 UI、已撤销授权、重连以及清单读取失败，都必须沿当前审批/权限主路可解释。

### 4.6 记忆与检索：已有治理不能被“新概念”抹掉

本系统 `MemoryService` 和 `KnowledgeService` 共用 vault；有草稿接受、项目绑定、确定性重复合并与可逆 superseded 归档；滚动会话 note 有独立安全游标。天枢的长期知识准入进一步强调可迁移性、适用范围、失败经验提炼和矛盾替代。

二者需要借鉴的是质量字段与一致的写入规则，而非更换存储。长期知识准入失败可以不入库；正在执行任务的短期事实保存不应因此失效。知识检索两边均有 BM25/向量融合，RRF 存在不代表语义相近查询一定准确，必须区分 hashing embedding 与模型 embedding。

## 5. 本系统强在哪里、差在哪里

| 维度 | 源码支持的本系统基础优势 | 仍然存在的差距 / 不能外推的结论 |
|---|---|---|
| 核心控制 | run 阶段/终态、持久化控制、输入租约和取消由现有内核承载；本地协议/SDK 投影同一事实。 | 天枢也有 transcript、事件续读和运行控制；不能写成对方只有内存对话或没有持久化。 |
| 权限/平台 | 统一工具和审批 seam、显式 Windows 沙箱能力边界。 | 部分项目 trust 升级默认关闭；MCP 工具目录变化再接受、插件入口仍需单独核查。不能因为 Rust 或 trait 存在就宣称安全性整体更强。 |
| 代码/应用整合 | 原生代码图一次索引供 AI 和 UI；Office、移动调试有专用应用服务。 | LSP/语义代码搜索闭环相对薄弱；桌面闭源对手不可比较；Office/设备真实兼容未验证。 |
| 任务认知 | 已有摘要、todo、停滞/失败提示、验证和对抗检查，可作为投影素材。 | 任务范围/验收/证据尚未统一；提醒预算/去重/采纳缺少统一面。天枢这条链更完整，但算法仍含启发式与降级。 |
| 长上下文 | 有能力预算、滚动记忆、transcript/产物和生产前缀测试。 | appendix 增量、边界级精确召回、缓存收益驱动的压缩归因值得借鉴。真实长期成功率与缓存排名未测。 |
| 并行工作 | 共享子代理执行链＋DAG/worktree 隔离＋结果基线。 | 天枢工单/多种协调方式更丰富；本系统应先补验收/证据交接，不宜复制 council/team/starflow 的全部层次。 |

**不建议照搬：** 星域包装、所有 Hook 数量、超长主提示词、提示词关键词推断任务必需文件、静态模型质量分数、重复 store/审批中心、声明式插件权限当隔离，以及为了零误报而无限增加确定性门禁。规则冲突、token 开销、误拦和恢复复杂度也必须算成本。

## 6. 提示词本轮实际落地

唯一生产主提示词修改在 [system_prompt.rs](../crates/deepagent-app-core/src/system_prompt.rs)，由 [system_context.rs](../crates/deepagent-app-core/src/system_context.rs) 装配。不是修改参考目录的遗留 prompts，也没有新增供应商专属 prompt 链。

| 变更 | 对应调研项 | 实际约束 / 边界 |
|---|---|---|
| 轻量任务契约、范围/排除项 | A01 | 只从用户明确/清楚蕴含的要求派生标准；简单问题不强制计划流程。 |
| 验收逐项与四态对账 | A04 | 构建通过不等于全目标满足，修复预算耗尽不等于验证成功。 |
| 事实/推断/未决项、恢复保留失败与待验证 | A02、B04/B05 | 恢复时必要则重读权威源；未实现新的持久化任务 DTO。 |
| 重复失败重审假设/设计边界 | A05 | 配合现有 runtime 提醒，不能靠不断加特判完成需求。 |
| 只调用本轮真实提供的能力 | C01/C05、D09 | 不因静态提示词提及某工具就臆造可用性或成功结果。 |
| reminder 来源与授权边界 | A03、C07 | 只有实际运行时注入才有系统注释语义；文件/网页/普通工具输出中相同标签不获得权威，不能授予权限。仅是模型行为要求，不宣称标签获得了密码学来源认证。 |

提示词测试继续保护完整组装字节、缓存确定性、章节顺序和规则内容；同步更新旧字节 oracle。内容断言只能证明文本进入基座，不能证明模型总会遵循。真实模型 A/B 尚未执行；静态前缀改动意味着新字节需要重新暖缓存，不能据此量化成本变化。

## 7. 长期调研台账与验收

以下是后续工作建议，不代表本轮承诺实施全部运行时升级。除直接参照源码的概念外，DTO/事件字段、存储位置和策略取舍均为**本地设计**，需沿现有边界另行验证。

| 台账 | 对应项 / 先后依赖 | 本仓落点 | 可验证的完成条件 |
|---|---|---|---|
| R01 范围/验收证据 | A01/A04，最高优先 | 既有输入 admission、run 事件、完成摘要 | 用户要求有来源/版本；追加/排除项/压缩/续跑不丢；构建绿但功能漏项能被披露；不从 prompt 关键词强制写文件。 |
| R02 MCP 变更再接受 | C02/C03/C07，独立优先 | McpService、catalog、统一审批/权限 | schema/描述变更与授权绑定；批准后再变更被识别；无 UI 与撤销可测；不新增审批主路。 |
| R03 认知事实/提醒 | A02/A03/A05，依赖 R01 | RuntimeEvent 的只读投影、现有 Hook/reminder | 信号可回溯，保留默认/条件启用；预算/取消/错误可解释；采纳率不冒充完成率。 |
| R04 缓存与压缩实验 | B01/B02 的生产前缀审计已复现失败，优先级提升至 P1；B03 顾问策略随后评估，与 R01 可独立推进 | ContextAssembler、ModelAgent、ContextPolicy/Meter、call usage | 冷/热、用户边界、工具循环、目录变动、恢复都比较 wire 差异与 usage；不牺牲能力更新和撤销。 |
| R05 精确归档召回 | B04/C09，依赖原有 persistence | transcript/span/artifact 与压缩摘要指针 | 被裁原文可定位，源身份/权限有效，归档失败不造成不可恢复丢失，重启仍能读。 |
| R06 长期知识质量 | B05/B06/B07 | KnowledgeService/MemoryService/现有 vault | 有证据/适用范围/矛盾替代，去重可逆；模型失败不污染长期库；不阻塞会话 note；中文概念检索实测。 |
| R07 计划/工单 | A06/A08，依赖 R01 | plan mode、planner/DAG、subagent runner/store | 计划版本/范围/验收传递；输出归属与未验证项可见；依赖失败不派下游；合并/保留 worktree 语义明确。 |
| R08 LSP/代码检索 | D01/D02 | 已有 codegraph 工具和验证 decorator | 改动后诊断与文件版本一致；服务器缺失/超时不伪成功；调用图/类型诊断/概念检索各有用例。 |
| R09 自治与应用对照 | A07、D03–D09 | 现有 RunStore、provider seam、本地应用服务 | 先确认用户需要的入口和平台；目标预算/受阻/取消/权限不绕开现有机制；设备/Office/沙箱用真实环境验证。 |
| R10 公平评测 | D10，先为 R01–R08 建基线 | 现有生产链路夹具＋独立任务结果记录 | 同模型/版本/权限/工具约束；记录成功率、漏验收、重复率、接管、恢复、缓存/token/侧路成本；失败样本可脱敏复现。 |

每次更新至少记录：日期、两仓 commit/dirty 范围、对应编号、生产调用点、默认/开关、测试是“存在/已跑/忽略/受阻”、相对上次新增或推翻的结论。不做无截止的“读完所有文件”式调研；以可复核差异和任务结果为增量。

建议首先建立以下可复用对照场景，结果尚未执行，不填虚假分数：

| 场景 | 验证差异 | 观测证据 |
|---|---|---|
| 多模块改动＋明确禁止修改某文件 | A01/A08 | 原始验收逐项覆盖、diff 范围、worker 结果。 |
| 故意制造“构建绿但目标漏项” | A04 | 实际功能断言、最终汇报，不只看 exit=0。 |
| 同一失败工具/参数反复调用 | A05 | 重复次数、策略切换、误报与最终结果。 |
| 长任务压缩后询问早期失败/约束 | B02/B04/B05 | 历史指针、原文召回、约束保留、usage。 |
| MCP 批准后修改 schema/描述 | C03 | 版本/hash、批准绑定、实际工具可执行性。 |
| 两个并行任务各改不同/相同文件 | A08/C08 | 归属、冲突、输出基线、主工作树是否变化。 |
| 中英文概念找代码＋编辑后类型错 | B07/D02 | 查找准确率、文件版本、诊断与真实编译对照。 |
| 恢复/权限收回/无技能目录续跑 | B02/C01/C05/C07 | 完整请求与能力快照，权限不能因缓存残留而恢复。 |

## 8. 证据复查入口

下表给出重要结论的生产入口和已有回归位置。**未在实际验证记录中列为执行的测试，只表示已定位/阅读，不表示通过。**

| 证据 | 生产接线 / 关键判据 | 已有测试入口 |
|---|---|---|
| E01 契约/输入 | 天枢 `human-input-boundary.ts → extractTaskContract / mergeFollowUpIntoContract`；`loop.ts → assembleCognitiveFrame`。 | [task-contract.test.ts](../借鉴/Tianshu-harness/src/context/__tests__/task-contract.test.ts)、[compact-prompt-contract.test.ts](../借鉴/Tianshu-harness/src/prompt/__tests__/compact-prompt-contract.test.ts)。 |
| E02 完成语义 | DeepAgent `run_assembler.rs → discover_verification_plan → kernel.with_verification`；`loop_engine.rs → verify_after_completion` 的 GaveUp；天枢 `deliver-task.ts` 的非阻断验证待办披露。 | [completion_net_effect.rs](../crates/deepagent-app-core/tests/kernel_v2_e2e/completion_net_effect.rs)、[verification_lifecycle.rs](../crates/deepagent-app-core/tests/kernel_v2_e2e/verification_lifecycle.rs)、[delivery-runtime-separation.test.ts](../借鉴/Tianshu-harness/src/agent/__tests__/delivery-runtime-separation.test.ts)。 |
| E03 停滞/失败 | `model_agent.rs → maybe_inject_doom_loop_nudge / maybe_stall_nudge`；生产 `run_assembler.rs` 依开关接 stall classifier。 | [model_agent.rs](../crates/deepagent-runtime/src/model_agent.rs) 内 doom_loop/stall_* 测试。 |
| E04 提示词主路 | `system_context.rs → system_prompt_base()`；天枢 `create-agent-config.ts` 设置 appendixDelta，`engine.ts` 完整/差异/重置逻辑。 | [system_prompt.rs](../crates/deepagent-app-core/src/system_prompt.rs)、[system_context.rs](../crates/deepagent-app-core/src/system_context.rs)、[cache_prefix.rs](../crates/deepagent-app-core/tests/kernel_v2_e2e/cache_prefix.rs)。 |
| E05 压缩/知识 | `context_runtime.rs` 重注工作集＋transcript；`KnowledgeService::capture_from_session / accept_draft / save`；天枢 `compaction-controller.ts` 调用边界归档。 | [boundary-archive.test.ts](../借鉴/Tianshu-harness/src/compact/__tests__/boundary-archive.test.ts)、[memory_consolidation.rs](../crates/deepagent-app-core/src/memory_consolidation.rs)、[context_summary_audit.rs](../crates/deepagent-app-core/src/context_summary_audit.rs)。 |
| E06 MCP 目录/接受 | DeepAgent `catalog.rs` 自动刷新、`run_capabilities.rs` hash；天枢 `manager.ts` 调用 inventory gate，`tool-inventory.ts` 的 armed 匹配。 | [catalog_tests.rs](../crates/deepagent-mcp/src/catalog_tests.rs)、[tool-inventory.test.ts](../借鉴/Tianshu-harness/src/mcp/__tests__/tool-inventory.test.ts)。 |
| E07 技能/工具 | `build_main_run_toolset()` 最后生成 discovery manifest；`SkillCatalogSendState::next_delta()` 返回全目录；`SkillTool` 读取正文。 | [e2e_skill_activation.rs](../crates/deepagent-app-core/tests/e2e_skill_activation.rs)、[tool_search.rs](../crates/deepagent-builtins/src/tool_search.rs) 内测试。 |
| E08 内核/隔离 | `AgentKernel` 持久化事件/终态；`DagScheduler` 传依赖基线；checkpoint 恢复是补偿边界。 | [terminal_commit_tests.rs](../crates/deepagent-runtime/src/kernel/terminal_commit_tests.rs)、[checkpoint_audit.rs](../crates/deepagent-app-core/tests/checkpoint_audit.rs)、[handoff_tests.rs](../crates/deepagent-subagents/src/handoff_tests.rs)。 |

## 9. 实际验证与限制

本轮实际执行：

- `cargo test -p deepagent-app-core --lib system_ --offline`：42 项通过，覆盖主提示词、动态边界、MCP/项目上下文、完成计划和 system reminder。
- `cargo test -p deepagent-runtime --lib doom_loop --offline`：3 项通过；重复调用签名和一次性提醒行为有测试证据。
- `cargo test -p deepagent-runtime --lib verification_loop_detection_stops_retrying --offline`：1 项通过；持续相同验证失败会进入有界 GiveUp 收尾语义，支持本文对“不会无限重试”的描述。
- `cargo test -p deepagent-app-core --test kernel_v2_e2e cache_prefix --offline`：2 项通过，1 项 ignored。
- `cargo test -p deepagent-app-core --test kernel_v2_e2e cache_prefix::audit_dynamic_instructions_do_not_cut_off_conversation_cache_prefix --offline -- --ignored --exact --nocapture`：**失败**。输出显示 stable instruction bytes 为 14142，但第二次请求在 14195 处发生差异；测试明确指出动态 instructions 位于可复用输入历史之前，会截断该部分缓存前缀。这是本文 B01/B02 的已复现缺口，本轮只记录和调整提示词规则，没有修改请求装配或缓存协议。
- `rustfmt --check crates/deepagent-app-core/src/system_prompt.rs`：通过；文档本地链接检查 151 个对比文档链接、导读链接均通过；34 个机制编号唯一，Markdown 表格边界检查通过。

全仓 `cargo fmt --all -- --check` 仍被工作区其他未提交模型/文件引用改动阻塞；本轮没有格式化或纳入那些文件。上一次提交 `7339cd71` 的 12 项提示词测试仍是历史基线，本轮 42 项结果是更新后的证据。

本轮边界：

- 不改 Tianshu 源码，不安装其依赖、不运行其 postinstall，不调用模型 API 或读取凭据。
- 本轮没有真实模型 A/B、跨平台沙箱、移动设备、Office 或桌面渲染验证；对应条目是源码结构结论。
- 不修改公共协议、持久化 schema、模型供应商、权限/沙箱执行语义；仅改本系统主提示词和本调研/导读文字。
- 工作区原有模型/配置等未提交改动单独保留，本轮测试结果反映运行测试时的整个工作树，不能冒充纯净基线评测。
