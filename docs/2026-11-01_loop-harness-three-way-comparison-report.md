# DeepAgent、Codex、Claude Code：Loop / Harness 对比与迭代方案

> 调研日期：2026-11-01（Asia/Shanghai）。范围为架构、执行语义和工程闭环，不是模型能力排名。
> 本文只提出迭代方案，不实施运行时、协议、权限或数据结构变更。

## 1. 阅读边界与证据基线

### 1.1 三套系统的基线

| 系统 | 本次基线 | 可以证明的范围 | 不能据此证明的范围 |
| --- | --- | --- | --- |
| DeepAgent Studio | `fdbef394344f1690df8d4c9c6d935420c129ff64` | 当前仓库的装配、生产调用链、类型与测试 | 真实模型任务成功率、未执行的 OS/网络联调 |
| Codex | 本地参考仓库 `a20fe6335f960a350483d0079db2ec281c68202c`，提交日期 2026-10-02 | 该开源快照的实现与测试设计 | 2026-11-01 所有发布渠道、云端和桌面私有实现完全相同 |
| Claude Code | 本地 npm 包声明 `@anthropic-ai/claude-code@2.1.88`，`restored-src` 为 source map 提取结果；另核对当前官方文档 | 该样本的实现；官方文档公开的当前外部契约 | 提取目录是官方源码发布、文件完整且未经修改、旧样本等于当前产品内部实现 |

证据标记：`C`＝本轮直接阅读源码；`T`＝实际执行测试；`D`＝本轮打开的官方文档；`I`＝根据证据作出的架构判断；`P`＝建议的本地设计，尚未实现。

**判定纪律：**

- “未发现”只针对本文明确检查的入口与目录，不以搜索不到一个名称断言整个产品没有某能力。
- “有实现”“接入生产”“对外暴露”“通过测试”“真实任务有效”分别判断，不互相代替。
- 安全拒绝、验证未通过、模型错误、用户取消不是同一种失败；不能套用同名枚举或 Hook。
- 不把 Codex/Claude Code 的 API 字段或默认值直接移植到 DeepSeek provider。
- 不复制参考工程业务代码；后续改造复用 `AgentKernel`、`RuntimeEngine`、`RunStore/run_events`、统一审批、取消、工具注册和 provider seam。

### 1.2 与既有报告的关系

[Claude Code 既有全景报告](claudecode-harness-research-report.md) 是历史研究，不作为本次差距判定的唯一证据。本次重新核对生产调用方，特别防止沿用“Cron 未接入”“没有并发子任务”“只有 messages 上下文”等已经可能失效的判断。本文结论只覆盖 loop / harness / loop engineering，不替代全产品安全审计。

### 1.3 文档形成方式

先固定版本与证据边界，再按输入、上下文、请求、工具、反馈、停止、恢复逐项比对；发现项必须连接到可核对的证据和后续实施、验收路径。此文件随本轮核对逐段更新，最终验证记录与未验证范围在文末统一列出。

## 2. 先给结论：不是缺一个循环，而是缺一致的完成与控制契约

DeepAgent 已有可用的 agent 内循环和相当完整的 harness 底座，不需要推倒重写。当前主要差距集中在：**验收结果与成功终态的关系、输入介入语义、机器事件粒度、SDK 流可靠性，以及跨 Run 的目标生命周期和评测闭环。**

| 结论 | 当前判定 | 证据／后续章节 |
| --- | --- | --- |
| 底座优势 | 多入口复用 Kernel、运行账本、审批和取消；自动验证也走同一工具权限链 | [DA-ASSEMBLY]、[DA-VERIFY-CMD]；第 6 节 |
| 已实测的弱项 | TS SDK 在 start 回复与事件同批到达时丢弃早期事件；默认 7 项测试不能覆盖此问题 | [DA-SDK-STREAM]、[DA-SDK-AUDIT]；F01 |
| 完成语义弱项 | 验证 GiveUp、事实门禁耗尽允许完成；执行成功不等于目标已通过验收 | [DA-COMPLETE]、[DA-VERIFY]；F02 |
| 控制与协议弱项 | `steer` 是取消后替换；`turn.started` 混入 Step；不能按 Codex 同名 API 理解 | [DA-STEER]、[DA-PROJECTION]；F03、F04 |
| 长任务缺项 | 有 Cron、工作流和单 Run 预算，但未发现持久 Goal 生命周期及其统一续跑控制 | [DA-CRON]、[DA-REQUESTS]；F08 |
| 工程闭环缺项 | 有测试、指标和审计夹具，但没有从真实失败到稳定评测门禁的完整可见闭环 | [DA-CI]、[DA-SDK-PACKAGE]；F09 |
| 可收敛的冗余 | 概念阶段漂移、重复完成分支、分散的重试／验证决策；不是数据库数量本身 | [DA-PHASE]、[DA-COMPLETE]；第 8 节 |

**优化顺序：先把“没丢事件、没假成功、没误取消、终态可对账”做实，再增加 Goal 或主动自动化。** 不以更多 Hook、更多复核模型、更多并发数作为成熟度指标。

## 3. 术语、层次与每一步的定义

### 3.1 三个概念不能混用

| 概念 | 本文定义 | 不是什么 |
| --- | --- | --- |
| Agent loop | 模型读取当前上下文，提出工具或最终回答，harness 执行工具并回灌事实，再决定继续或停止的运行闭环 | 不是模型内部思维过程；不是固定先写计划再编码的流水线 |
| Harness | 包围模型的执行宿主：装配输入／上下文、提供工具、权限与沙箱、处理审批／取消、记账、持久化、恢复、协议和界面投影 | 不是一个提示词；不是另外再造一套 runtime |
| Loop engineering | 设计触发、状态、行动、反馈、验证、停止、恢复和预算，使循环可以稳定重复；还包括用运行证据改进这些机制 | 不是把 `while true` 包住模型；不是无限自动续跑 |

Claude 官方把概念 loop 表述为“收集上下文—行动—验证”，并明确三个阶段会交织，harness 负责工具与上下文。[CC-WORKS] Anthropic 的 loop engineering 文章按触发／停止区分 turn-based、goal-based、time-based、proactive；OpenAI 的 improvement-loop 示例强调 traces、反馈、eval 和实现之间的闭环。两者并非同一份状态机规范。[CC-LOOP-ENG] [OA-IMPROVEMENT]

本文因此分开讨论：**运行时如何循环（第 3 节）、harness 如何承载（第 4 节）、外层如何持续工作（第 5 节）、开发者如何持续优化循环（第 10 节）。** 后者的步骤表是本文归纳，不声称三套产品有同名内部模块。

### 3.2 运行粒度的对应关系

| 粒度 | DeepAgent | Codex 本地快照 | Claude Code／Agent SDK |
| --- | --- | --- | --- |
| 持久会话 | Session；harness 对外称 Thread | Thread／Session | Session／transcript |
| 一次被接纳的用户工作 | Run；app-server 的 `turnId` 对应一次运行身份 | 用户 Turn，可包含多次采样和工具往返 | 交互语境常称用户 turn；SDK 文档的 turn 另指一次模型输出及其工具反馈周期 |
| 内循环一次决策 | Runtime Step，`Agent::think(step, observations)` | 采样／StepContext；不与用户 Turn 等同 | `queryLoop` 的一轮；SDK `maxTurns` 计数按其自己定义 |
| 模型请求尝试 | 一个 Step 内可有 retry、fallback、compact 后重试 | 一个 sampling request 内可重试流或处理抢占 | 一轮 query 内可有流回退／输出上限恢复等 |
| 一次具体行动 | Tool invocation，关联 `call_id` 与执行记录 | Tool call，捕获所广告工具对应的 StepContext | `tool_use` 与 `tool_result` 配对 |
| 跨运行目标 | 尚无本文检查范围内的统一持久 Goal | 可启用的 thread Goal extension | 当前官方 `/goal` 是 session 级外环；不以 2.1.88 样本证明当前实现 |

依据：[DA-MODEL]、[DA-LOOP]、[CX-TURN]、[CX-PARALLEL]、[CC-QUERY]、[CC-SDK-LOOP]。**不能直接比较三者的 max turns 数字，也不能把模型 `response.completed` 当作用户工作已完成。**

### 3.3 逐步定义与三方对照

下表是分析视角的步骤分解，不要求每次运行严格按行执行。检查、压缩、保存可以跨步骤发生；只读问答也不一定需要构建验证。

| 步骤及定义 | DeepAgent 当前实现 | Codex 当前参考快照 | Claude Code 样本／当前契约 |
| --- | --- | --- | --- |
| 1. 接纳：确定会话、运行身份、输入所有者和有效配置 | ChatService／RunCoordinator／输入 lease；app-server 忙时普通 start 拒绝；新运行装配进 Kernel | Thread 接纳 Turn；steer 校验当前 Turn；Turn／StepContext 固定执行配置 | query 接受输入和 ToolUseContext；外层交互／SDK 管理 session 与输入；不能只从 query 推断全部接纳规则 |
| 2. 恢复与建上下文：形成模型可见、可追踪的事实历史 | Response items 为权威，Message 是兼容／展示投影；附工具 schema、记忆、提醒 | 从 rollout 重建历史、配置和 context-window 元数据；装配 skills／工具 | 样本 messages、attachments、skills／memory；当前会话可从 transcript 恢复 |
| 3. 压力管理：在有限窗口内保留关键约束与工具配对 | 预检查、microcompact、预取／后台压缩、溢出恢复；不是只在 Step 尾部 compact | 采样前 compact；窗口 rollover／压缩后继续；有可重建的压缩边界 | snip／microcompact／autocompact，加 reactive compact；压缩与停止重试共享部分恢复标记 |
| 4. 推理／请求：向 provider 发送上下文并解析结构化输出 | ModelAgent attempt 子循环；保留 reasoning、usage、工具增量；普通 attempt 默认 3，但另有恢复分支 | `run_sampling_request`；流重试、context／usage 错误分类、输入抢占 | query 调用模型流；工具 block 决定是否有工具续跑，不能仅信 `stop_reason` |
| 5. 行动裁决：调用前确认 schema、Hook、权限、审批、沙箱 | ToolExecutionPipeline／registry／统一审批与取消；不能由 SDK 绕开 | ToolCallRuntime → router／orchestrator；集中审批、沙箱选择、受约束的重试 | `runToolUse` → PreToolUse／permission decision → 执行；不是 streaming executor 绕开审批 |
| 6. 执行与调度：安全并发，写入／互斥动作建立屏障 | 标记 concurrency-safe 的工具分组并发；非安全工具顺序屏障；投机提前执行有额外限制 | 并行工具持读锁，非并行工具持写锁；流中形成工具 future，再有序收集结果 | StreamingToolExecutor 调度安全工具；非安全工具互斥，并对子工具／同组失败处理取消 |
| 7. 观察：将真实执行结果配回请求，驱动下一轮 | Observation 连同 call_id 回灌权威历史；错误也属于事实反馈 | 记录 response/tool output、executed call 信息；计算是否需要 follow-up | tool_result 配对；中断／回退也补对应结果；用户队列不插入未完成配对之间 |
| 8. 验证：对产物／目标收集独立证据 | 编辑后检查＋完成构建计划＋文件事实门禁＋可选对抗复核；适用范围和约束强度不同 | 普通 Turn 可用工具验证、Stop Hook 反馈；所读主循环未见所有任务统一强制跑项目全测试 | 普通 loop 的 verify 是模型选工具行为；可配置 Stop checks；当前 Goal 有独立 evaluator，但它不自行读取文件或运行命令 |
| 9. 反思与续跑：将失败转成下一步约束，控制重复成本 | 验证失败签名／重试上限、重复调用提醒、可选 stall classifier；很多是 advisory | `needs_follow_up` 合并模型续跑与 pending input；Stop 反馈可再循环 | 工具结果、输出截断恢复、Stop blocking errors 转下一轮；当前 Goal 另判 met／not met／impossible |
| 10. 停止裁决：区分候选回答、验收满足和不能继续 | Complete → 验证／事实门禁／BeforeResponse／Stop；部分耗尽 fail-open；NeedsApproval 可终结为 Blocked | 无 follow-up 后运行 Stop hooks，有有效阻止反馈则再循环；取消与请求错误分开处理 | 无工具不必立刻退出，还需 Stop hooks／错误处理；当前 Stop 具有连续续跑上限 |
| 11. 提交终态与投影：给出可对账的最终结果 | Kernel 统一 TerminalKind；RunStore 事务一次落终态，确认提交后投影；文本增量可聚合 | rollout／状态／事件投影各司其职；app-server 输出 Turn／Item 生命周期 | transcript 持续记录；SDK 用 ResultMessage subtype 区分结果；不把任意 assistant 文本当成功凭据 |
| 12. 恢复或再触发：在安全边界继续后续工作 | 重启收束 orphan Run，恢复会话／checkpoint；Cron 可触发新 session；不是恢复原执行栈 | resume 重建可用上下文；可启用 Goal 在 idle 时发起新 Turn | resume 恢复历史；当前 Goal／schedule 有明确恢复规则；未完成外部工具并不任意重放 |

本表源码锚点：[DA-ASSEMBLY] [DA-MODEL] [DA-LOOP] [DA-VERIFY] [DA-PROJECTION] [DA-RECOVERY] [CX-TURN] [CX-SAMPLING] [CX-PARALLEL] [CX-ORCHESTRATOR] [CX-REPLAY] [CC-QUERY] [CC-TOOLS] [CC-EXECUTOR]；官方补充：[CX-APP-SERVER] [CC-SDK-LOOP] [CC-GOAL] [CC-HOOKS] [CC-SESSIONS]。

### 3.4 本系统“九阶段”的真实位置

| 名称 | 定义 | 生产执行中的实际位置 |
| --- | --- | --- |
| LoadSession | 读取／恢复会话 | 运行装配／恢复入口，不是每个 Step 都重新加载 |
| BuildContext | 装配模型上下文 | Run 装配与 ModelAgent 每次请求投影共同完成 |
| Think | 模型决定下一步 | Runtime 调用 Agent，ModelAgent 内还有多请求尝试 |
| Execute | 执行工具 | 根据 AgentDecision 分支进入统一工具链，可一批多工具 |
| Observe | 接收执行反馈 | Observation 回灌下一次决策，成功／拒绝／取消都要有明确表示 |
| Verify | 检查变更／产物 | 编辑后和完成候选处按条件执行，不是所有 Step 的必经点 |
| Reflect | 诊断失败并选择行动 | 验证失败等路径；GiveUp 当前是停止修复并接受完成，而非验证通过 |
| Compact | 控制上下文增长 | 请求前、背景工作和出错恢复均可能发生 |
| Save | 保存事实／执行状态 | 事件过程中持续发生，终态另有事务提交，不只尾部保存 |

`LoopPhase::ORDER` 目前只见定义、测试和 re-export，未见生产消费者；注释所称“每轮按阶段推进”不能视作已实现事实。`RunPhase` 则由 `phase_rank` 单调更新，是运行进度高水位，不是 Think ↔ Execute 往返的实时游标。[DA-PHASE] [DA-KERNEL]

因此后续应先统一术语与事件，而不是把真实循环硬塞进九个串行阶段。若旧图把九阶段画成固定状态机，应按本节修正其含义。

## 4. Harness 设计的边界对照

### 4.1 主链与责任归属

| 维度 | DeepAgent | Codex | Claude Code | 对本系统的判断 |
| --- | --- | --- | --- | --- |
| 共用执行核心 | Desktop／CLI／app-server → ChatService → RunAssembler → AgentKernel → RuntimeEngine → Agent；WorkflowAgent 也进 Kernel | app-server／交互宿主驱动 core Thread／Session／run_turn | queryLoop 是模型／工具内循环；当前 Agent SDK 复用 Claude Code binary 能力 | 已有正确收敛方向，不缺第二套引擎 |
| 机器协议 | 独立 harness-protocol 只管 DTO 和单向事件投影；stdio RPC、CLI JSONL、Rust／TS SDK 已有实现 | app-server Thread／Turn／Item 与主动通知契约 | SDK／非交互消息协议，不能当作 Codex JSON-RPC 同构物 | 应补语义完整性，不追求字段同名 |
| 存储 | Session EventStore 管会话；RunStore／run_events 管运行账本；控制／审批记录可对账 | rollout、context reconstruction、state 数据各有用途 | transcript 与 session restore；SDK 有结果与会话身份 | 多种持久化用途不自动等于冗余 |
| 工具与安全 | Tool trait／registry、MCP adapter／deferred、统一 pipeline、approval 和 SandboxBackend | tools router／orchestrator 管审批、沙箱和 retry | runToolUse 管 Hook／permission；shell sandbox 只是其中一个边界 | 不按工具数量判优劣，也不把 permission 当 OS 隔离 |
| 终态与取消 | Kernel／CancellationTree／RunStore；已有 deadline cleanup 与 durable terminal | Turn cancel／preempt 与 tool cancellation；协议把 interrupted 单独表达 | query abort／tool child abort；SDK Result subtype；权限拒绝与 API error 分路 | 本系统底座具备，外部解释仍要进一步统一 |
| 平台适配 | 原生 Windows 工作台，Sandboxie／Windows Sandbox／Direct 后端边界 | 本轮按工具 orchestrator 评价，不对其所有 OS 后端作完整审计 | 当前官方：shell sandbox 支持 macOS／Linux／WSL2，原生 Windows 命令无此沙箱 | 本系统有 Windows 差异化位置，但需真实 OS 验证，不等于天然更安全 |

依据：[DA-ASSEMBLY] [DA-HARNESS] [DA-RUNSTORE] [DA-COORDINATOR] [DA-SANDBOX] [CX-TURN] [CX-ORCHESTRATOR] [CX-REPLAY] [CC-TOOLS] [CC-SDK] [CC-SANDBOX]。

### 4.2 哪些“成熟设计”值得学，哪些不能照搬

- **学 Codex 的身份与生命周期边界：** Thread、用户 Turn、Item、请求 attempt、pending input 分层；工具使用其广告时的 StepContext；不是把所有事件都叫 Turn。[CX-STEER] [CX-PARALLEL]
- **学 Claude 的工具配对与回退清理：** 先保证 tool_use／tool_result 闭合，再吸收队列；fallback 清理旧 partial／executor；不能把失败 attempt 残片当新请求的有效事实。[CC-QUERY] [CC-EXECUTOR]
- **学双方“控制面不等于模型判断”：** 主动触发、预算、审批、暂停和恢复由宿主约束；模型可以提供判断，但不应凭一句“完成”替代持久终态或验收证据。[CX-GOAL-SOURCE] [CC-GOAL]
- **不要照搬供应商适配：** 本系统以 DeepSeek 为默认方向，内部 Response items 是自身规范化 seam；对外 provider 字段、reasoning、cache usage、tool delta 必须显式映射，不能以 Codex 协议替代 DeepSeek 语义。
- **不要照搬研究示例为默认产品流程：** Anthropic initializer／coding agent、进度文件与 feature list 是长任务 harness 的公开实验方案，不证明每次 Claude Code 问答都跑两代理；OpenAI improvement-loop 是开发流程示例，也不证明 Codex 默认会自动修改、合并和部署自身。[AN-LONG-HARNESS] [OA-IMPROVEMENT]

## 5. Loop engineering：四类工作外环与控制差异

### 5.1 触发与停止的对比

| 外环类型／定义 | DeepAgent | Codex | Claude Code | 迭代含义 |
| --- | --- | --- | --- | --- |
| 用户驱动：用户输入触发，一次工作停在结果／阻塞／边界 | Kernel Run 已具备 | 普通 Turn 已具备 | 普通 query／用户输入已具备 | 基础内循环不是主要缺项 |
| 目标驱动：目标未满足时，跨工作单元继续，受预算和用户控制 | Verification／adversarial 在单 Run 内；未发现持久 Goal 状态与安全 idle 调度闭环 | 可启用的 thread Goal：持久状态、记账、idle continuation；不能把预算耗尽标完成 | 当前 `/goal`：独立 evaluator 使用对话证据，未满足则续跑；可恢复 active condition | 新增的是外层目标控制契约，不是第二个工具循环 |
| 时间驱动：计划到期触发，不代表上次任务未完成 | CronService 文件存储；Desktop 已启动 tick；每次 fire 新 session；CLI 查到注册但未见 tick 驱动 | 本轮未审计 Codex 桌面调度实现；不能由 Goal 推断它拥有 Cron 的所有语义 | 当前 `/loop`／Cron 在 session idle 边界派发；另有独立 Desktop／cloud schedules | 区分“新任务定时启动”与“同一任务继续”；不宣称 DeepAgent 没有 Cron |
| 事件／主动驱动：外部变化到来后，经授权、去重、接纳再触发 | 已有 workflow、子代理和控制中心；本轮检查入口未发现通用事件触发→去重→续跑→验收产品闭环 | 开放 harness 可由调用方集成；本轮不宣称任何外部事件默认开启 | 官方 loop engineering 和 dynamic workflow 展示组合模式 | 是受控产品能力，不是让后台一直轮询模型 |

来源：[DA-CRON] [DA-DESKTOP-CRON] [DA-CLI-CRON] [CX-GOAL-SOURCE] [CX-GOALS] [CC-GOAL] [CC-SCHEDULE] [CC-LOOP-ENG] [CC-DYNAMIC]。

### 5.2 目标、计时器与工作流不可互相冒充

本系统的 **Cron 是调度，Workflow 是执行结构，Verification 是证据检查，Goal 应是跨 Run 的目标契约**。已有其中三种，不代表第四种自动存在；也不应新增一个“GoalEngine”复制 Kernel 的工具、审批、存储和取消。

Codex 本地 extension 的 `continue_if_idle` 在目标状态锁下读持久目标，再通过 Thread 的 `start_turn_if_idle` 接纳工作，不是脱离 Thread 自行发模型请求。[CX-GOAL-SOURCE] Claude 当前 Goal 借助 session-scoped Stop evaluator，且 evaluator 不独立执行工具；因此“另一个模型判通过”仍不能取代真实构建、产物或外部状态证据。[CC-GOAL]

本报告建议的统一原则（`P`）：**所有外环只产生“下一次输入意图”，由既有 AppCore 接纳；执行与终态仍归 Kernel。** 用户输入、取消、待审批和正在收尾的 Run 优先；无进展／超预算必须停，不能自动重试不可确认的外部副作用。

## 6. 本系统的优势与必须保留的资产

这里的“优势”区分为已有同类能力、面向本产品的架构优势和明确平台差异；不等于已经在成功率、速度或安全性上胜过两个参照产品。

| 优势项 | 已核实事实 | 对比后的价值与边界 |
| --- | --- | --- |
| 统一内核而非入口各写循环 | 普通聊天与 WorkflowAgent 同进 AgentKernel；harness 协议不拥有执行／审批／取消 | 与成熟 harness 的共核方向一致；适合把 Desktop 能力平台化，不应为了 SDK 绕开它。[DA-ASSEMBLY] [DA-WORKFLOW] [DA-HARNESS] |
| 可审计的运行账本和终态提交 | RunStore 事务落终态；Kernel 先提交再发 canonical terminal；有失败注入测试 | 可把 UI、JSONL、SDK 的最终状态统一到一份证据；本轮 5 项 terminal commit 测试通过，但不证明外部副作用 exactly-once。[DA-RUNSTORE] [DA-TERMINAL-TEST] |
| 验证复用权限链 | 自动验证命令通过现有 bash／ToolExecutionPipeline，使用该 Run 的 Hook、approval、cancel、workspace 边界 | 这是应保留的执行边界；比新增一个无审批“验证命令执行器”更适合持续演进。不是声称 Codex／Claude 缺权限保护。[DA-VERIFY-CMD] [DA-PIPELINE] |
| 验证与反思已有结构化 seam | VerificationPlan、ReflectionEngine、文件事实检查、可选复核均存在 | 不是从零补 verify，而是收敛其证据、强度和耗尽语义；自动发现优先轻量构建检查，并不自动覆盖业务验收。[DA-VERIFY] [DA-REFLECT] [DA-PLAN] |
| 面向本地 Windows 的后端可插拔性 | SandboxBackendRegistry 显式选择 Direct／Sandboxie／WindowsSandbox，不可用的显式后端拒绝 | 相对 Claude 当前原生 Windows shell 无沙箱的公开限制，有明确产品差异化空间；只证明适配边界存在，未证明隔离强度或实际兼容性更高。[DA-SANDBOX] [CC-SANDBOX] |
| Provider 与产品运行时可以分别演进 | ModelAgent 通过 models seam 使用结构化 Response items；权威历史不只剩展示文本 | 适合围绕 DeepSeek 做供应商映射、usage／reasoning 保真；需要 provider 契约测试，不据此宣称任意模型都即插即用。[DA-MODEL] |
| 已有真实工程问题的审计资产 | SDK 的 burst fixture、Rust durable terminal／验证测试等已经入库 | 当前短板是把它们变成必跑门禁，而非再建一套庞大测试系统。[DA-SDK-AUDIT] [DA-TERMINAL-TEST] |

## 7. 弱项、缺失与根因：Evidence → Finding → Path

优先级含义：`P0`＝先保证结果与控制可信；`P1`＝收敛策略与使用契约；`P2`＝扩展长期目标、主动执行和效果优化。它不是漏洞严重性或承诺工期。

### F01 · P0：高层 SDK 会丢失已经收到的早期事件

- **Evidence／状态：`C + T`，本轮可重复验证。** `runStreamed` 先订阅，但在 `await this.run()` 后才设置 `turnId`；回调在此前只忽略事件而不缓冲。[DA-SDK-STREAM] 默认 `pnpm test` 7 passed；显式开启仓库已有 early-events 审计后，controlled 通过，event-first／response-first 均失败，结果为 1 passed / 2 failed。[DA-SDK-AUDIT]
- **Finding／影响：** 原始 observer 收到 `item.updated` 和原始 `turn.completed`，同一 SDK 的高层 stream 在 barrier 前却是空列表；fixture 额外发出的 cleanup terminal 才释放等待。真实服务是否每次触发并未测定，但客户端对合法同批到达顺序的丢失已经确定。
- **根因：** 运行身份的建立与订阅交付之间没有原子接缝；“先 subscribe”不足以解决尚未知 turnId 的事件归属。
- **Path／建议：** 在现有 transport／subscription 内设计有界 pre-start 缓冲并在身份确定后分流，或由接纳协议提供可先绑定的身份；start 失败清理自己的缓冲，不重发工具／输入，不牺牲其他线程隔离。具体方案需先测试再选，不新增 event store。
- **验收：** 两种单批顺序均保留原始事件并以原始 terminal 结束；无额外 cleanup 事件也结束；并发 thread、start 拒绝、断连、取消、buffer 上限都不串流／泄漏。将本夹具纳入默认 CI。
- **兼容：** 不改变 wire shape 也可修复高层 SDK；buffer 必须有大小与清理边界，不能靠 sleep 或“服务端晚点发”遮蔽。

### F02 · P0：停止修复、执行结束、验收成功尚未形成清晰分离的契约

- **Evidence／状态：`C + T`。** `verify_after_completion` 把 GiveUp 转为 GaveUp，Complete 两分支接受；事实门禁耗尽记录 Verification false 后也完成。[DA-COMPLETE] [DA-VERIFY] 本轮 `verification_loop_detection_stops_retrying` 通过，其断言本来就是“持续验证失败，最终仍 Completed”。这是当前设计，不是测试证明验收通过。[DA-VERIFY-TEST]
- **Finding／影响：** 自动化若只看完成事件或成功状态，可能把 best-effort 退出当成验收达成；尤其不能把 `cargo check` 通过当成业务测试通过。另一方面，缺环境／工具链不应被一律判成模型失败。
- **根因：** 启发式保护默认从宽，与显式验收条件共用“接受完成”；现有 `VerificationPolicy::Strict` 只影响编辑后工具结果，不能理解为整次 Run 的严格验收。[DA-DECORATOR] `into_completion_result()` 对 Cancelled／Blocked／MaxTurns 也可返回 Ok；装配方已另取 `terminal.succeeded()`，所以不能据此断言所有 UI 已误报。[DA-KERNEL] [DA-ASSEMBLY]
- **Path／建议：** 先建立统一完成摘要，明确执行终态、验收状态、证据出处、未检查原因与允许停止原因。把自动发现检查保持 advisory；用户／工作流显式声明的必需条件走明确的强约束策略。修复耗尽应成为“停止但验收未满足”，不能自动升级成“已满足”。这是本地设计，字段与版本需另行评审。
- **验收：** 通过／失败／跳过／环境缺失／超时／取消／预算耗尽分别覆盖；必需条件失败不能显示“验收通过”；CLI、SDK、Desktop、replay 看到相同结论；纯问答不被强迫运行无关构建。
- **兼容：** 优先增量发布版本化摘要并更新消费者；旧成功语义不能悄悄改成不同枚举，否则会影响历史回放、脚本退出码与下游工作流。

### F03 · P0：Run、Step、phase 在机器投影里存在粒度混淆

- **Evidence／状态：`C`。** Runtime RunStarted 和 Runtime TurnStarted `{ step }` 都投影为 harness `turn.started`；文本／reasoning item 更新没有稳定 itemId。[DA-PROJECTION] `LoopPhase` 没有生产消费者，RunPhase 又只能升不降。[DA-PHASE] [DA-KERNEL]
- **Finding／影响：** 客户端可能重复建 Turn、错误计算轮数、把进度高水位当实时状态；事件可以序列化不等于生命周期正确。
- **对照：** Codex 的用户 Turn 内可反复采样／执行工具；其 steer 不发新 turn started；Claude SDK 又明确用另一种 turn 计数，说明必须先定义本系统粒度。[CX-TURN] [CX-APP-SERVER] [CC-SDK-LOOP]
- **Path／建议：** 一个已接纳 Run 只产生一次协议 Turn start；Step／attempt／活动状态使用独立投影或带明确类别的 item，保留现有内部事件为权威；内容／reasoning 更新需有可关联到最终产物的稳定 item 身份；RunPhase 文档写清高水位。先做事件语义契约，再决定是否弃用 LoopPhase，不为枚举补一套假执行机。
- **验收：** 两次模型调用、工具并发、验证重试、压缩、取消、replay 的 start／terminal 数量与身份一致；新旧协议 golden fixture 同时可解释。
- **兼容：** 影响事件消费者和历史数据解释，需要协议版本／能力协商；不能直接重命名旧日志字段然后假定所有消费者同步。

### F04 · P1：`turn/steer` 名称相同，语义却是 replacement

- **Evidence／状态：`C + D`。** 本系统先 request_cancel，记录 continuation，再生成新 turnId／admit_run，返回 replacesTurnId。[DA-STEER] Codex 则只接纳匹配当前 Turn 的输入，不新建 Turn。[CX-STEER] [CX-APP-SERVER] 本系统 README 已明确说明 replacement，问题不是完全没有文档。[DA-SDK-README]
- **Finding／影响：** 想补充上下文时可能中断当前工具、验证或审批；迁移 Codex 客户端时会错误期待同 Run 继续；已执行副作用不会因 replacement 自动撤销。
- **根因：** 用取消／重启组合实现了用户介入，但 API 名称暗示原地追加。已有输入 lease 和 dispatch 所有权应复用，不能绕开它们直接改 messages。
- **Path／建议：** 立即明确 replacement capability／文档；后续若做真正 steer，在安全模型／工具配对边界消费输入，校验 expected active identity、持久记录接纳与消费，并保持同一 Run 的权限快照。保留显式 replace 作为另一动作，不静默改变旧方法。
- **验收：** 流中、工具运行、待审批、验证中、terminal 竞态、旧身份、重复请求分别测试；输入恰好消费一次或明确拒绝；不产生幽灵 Run，不把未知副作用自动重试。
- **兼容：** 原地 steer 属行为变更，版本化迁移；本文没有实施。

### F05 · P1：Stop 续跑缺独立预算，Hook 名称也有兼容陷阱

- **Evidence／状态：`C + D`。** 本系统 BeforeResponse／Stop 的 Ask／Deny 返回 Retry；本处未见独立 Stop continuation counter；全 Run 仍有默认 30 分钟 deadline，因此不是无边界永远运行。[DA-STOP] [DA-CONFIG] 当前 Claude 文档有连续 8 次 Stop 续跑 cap，工具调用后重置；该现行数字不倒写成 2.1.88 样本事实。[CC-HOOKS]
- **Finding／影响：** 自相矛盾的 Stop hook 可在全局 deadline 之前反复花费模型请求；局部完成门禁的 max retries 不能限制这条独立路径。
- **语义差异：** 本系统 StopFailure 在完成候选被 Hook 拒绝时触发；Claude 当前语义是 API 错误结束时的通知，无决策权。不能承诺同名插件无损兼容。[DA-STOP] [CC-HOOKS]
- **Path／建议：** 在同一 Run 控制策略中记录 Stop 连续拒绝、总花费、同反馈签名和真实进展；限额耗尽有明确退出理由，并遵守 F02 的必需验收语义。Hook 增加版本／事件语义说明；如提供兼容映射，区分完成被拒与 provider failure。
- **验收：** 永远 deny、空反馈、交替 deny、API 错误、取消、工具推进后重置分别有界；达到 cap 不被标成“必需验收已通过”。
- **不照搬：** Codex 所读 Stop 分支也有 continuation，本文未确认它与 Claude 同样的独立 cap，不把所有参考实现理想化。[CX-TURN]

### F06 · P1：完成路径重复，复核能力与输出表示耦合

- **Evidence／状态：`C`。** Complete 和 CompleteMessage 都重复验证／事实门禁／Hook／保存／结束；可选 adversarial 只在 CompleteMessage 分支；CompleteItems 先归一到该分支。[DA-COMPLETE]
- **Finding／影响：** 不同 Agent 的完成表示可能走不同策略；后续修一处漏一处，已经存在实质策略差异，不只是函数长。
- **证据不足的复核：** 当前 AppCore goal-coverage 路径给复核者的是目标、变更路径和最终回答，不是完整 diff、命令退出码与产物事实。多个复核者数量增加，不能补足同源缺证据。[DA-PANEL] 这也不是“本系统没有对抗验证”，而是已有验证的证据上限。
- **Path／建议：** 将候选回答先规范化为保留 Message／Response items 的统一内部完成输入，再复用一个完成决策函数；分离事实必需项与 advisory panel。复核只消费脱敏、受限的已有执行证据包；若需要读工具也必须走现有权限链，不默认新增执行权。
- **验收：** 三种完成表示在同样前提下有相同验证、Hook 次数和终态；provider items 不重复落盘；富内容不丢；复核明确区分支持／反驳／无法判断，实际命令证据优先。
- **兼容：** 不借重构重写供应商历史表示，不取消现有输出类型的外部兼容。

### F07 · P1：恢复能力存在，但高层续读、分页和副作用对账尚不完整

- **Evidence／状态：`C`。** `thread/read` 有 session／每 Run cursor 和 action／approval 投影，但先载入整个 session 再过滤，且遍历各 Run 读取事件；未见本请求的 page-size 上限。[DA-READ] SDK README 明确不自动 reconcile stream／pending approvals。[DA-SDK-README] 启动恢复收束 orphan Run，不恢复模型执行栈。[DA-RECOVERY]
- **Finding／影响：** 长会话重连开销不可控；SDK 调用者要用底层请求自行对账；误把“恢复会话”当“从崩溃指令继续”，可能重复外部行动。
- **对照：** Codex 有能围绕有效压缩边界重建上下文的 rollout 扫描；Claude 当前恢复规则明确未完成工具不直接 rerun。二者都不能据此被宣称恢复任意进程／网络副作用。[CX-REPLAY] [CC-SESSIONS]
- **Path／建议：** 基于现有 store 加有界读取和稳定 cursor；SDK 提供 typed read／resume／reconcile；保留 transport receipt 与 durable cursor 的区别。对“已提交／未开始／可能已执行但结果未知”作显式对账，只有确认可重入／幂等的动作才允许续做。
- **验收：** 大历史分页有界；断点／压缩／重连无重复漏读；ack 重置不影响 durable cursor；断连中待审批、工具已执行未记录、已提交 terminal 都有明确恢复结果。
- **兼容：** 不新建 replay store；恢复执行若将来实现，须独立授权／版本定义，不把当前 orphan cleanup 改名为 auto-resume。

### F08 · P2：缺少持久目标与跨 Run 续跑的统一契约

- **Evidence／状态：`C + I`，范围限定。** 检查 runtime、AppCore、harness request／SDK 入口，找到单 Run verification／goal-coverage 与 Cron，未发现对应 thread Goal 的状态、协议和接纳闭环；不是仅因某个类型名不存在。[DA-CONFIG] [DA-PANEL] [DA-REQUESTS] [DA-CRON]
- **Finding／影响：** 长任务的目标、验收、暂停、预算和下一步容易只在上下文里；每次新 Run 或定时新 session 都需重建目标，难审计何时允许自动继续。
- **对照：** Codex 的 Goal 是持久 thread 状态及闲时接纳；Claude 当前也有 session Goal evaluator／恢复规则。不能再把“Goal”算成 Codex 独有能力，也不能把 Claude 旧样本未覆盖写成当前产品没有。[CX-GOAL-SOURCE] [CX-GOALS] [CC-GOAL]
- **Path／建议：** 等 F01–F07 的结果／接纳边界稳定后，将显式目标、验收引用、预算记账和生命周期挂到现有 session／control 数据；在 Run terminal 后经既有 coordinator 检查 idle 再接纳下一输入。目标预算耗尽≠目标完成；用户暂停／取消有优先权；无工具／无证据进展须有界停止。
- **验收：** 有用户排队、待审批、后台工作、正在 cleanup 时不自动抢跑；崩溃恢复不忘预算；重复调度不双发；不能因模型一句 completed 直接关闭必需验收目标。
- **不应新增：** 第二套 RunStore、Goal 专属工具注册表、无人值守默认批准或脱离 Kernel 的模型循环。

### F09 · P0／P2：审计样例存在，但未成为稳定的工程反馈门禁

- **Evidence／状态：`C + T`。** SDK `test` 只运行 sdk.test.mjs，early-events 错误场景还需 `RUN_HARNESS_AUDIT=1`；所读 CI 有 Rust workspace 和 Desktop build，但无 packages/sdk 的独立测试步骤。[DA-SDK-PACKAGE] [DA-SDK-AUDIT] [DA-CI] 已有 metrics／事件不是“完全没有观测”。[DA-METRICS]
- **Finding／影响：** 默认全绿可与确定的流丢失共存；只靠人工报告发现问题，会持续增加特判而不形成稳定回归。源码／协议测试不能直接证明端到端目标成功率。
- **Path／建议：** P0 先把已确认可重复、无凭据依赖的 harness 审计加入门禁；对仍失败的用例建明确修复项，不简单解除所有 ignore。P2 再用脱敏真实轨迹→失败分类→回归任务／grader→受控实验→版本对比建立效果闭环。[OA-IMPROVEMENT]
- **验收：** F01 两个反例修复后常态运行；门禁失败不能发布“全部通过”；保留失败输入与版本。模型 eval 与确定性协议测试分开跑，后者不依赖真实模型波动。
- **边界：** 未掌握两个参照产品全部内部 eval／发布流程，不做“它们测试全绿、我方完全无 eval”的断言。

### F10 · P1：重试与防空转机制多，但缺少统一可解释的进展决策

- **Evidence／状态：`C + I`。** 普通模型 attempt、输出上限恢复、压缩恢复、验证重复签名、完成事实修复、adversarial、stall nudge 各有机制。重复工具 nudge 比较工具名＋参数签名，不比较工具输出内容；classifier 是一次 Run 一次 advisory，失败放行。[DA-MODEL] [DA-STALL] [DA-REFLECT] [DA-CONFIG]
- **Finding／影响：** 单看 `max_model_attempts=3` 无法解释总请求量；连续成功的同参数轮询不一定空转，换参数也不等于有进展；“多加一个反思提醒”难以给出成本上界和原因。
- **Path／建议：** 保留局部恢复知识，但集中记录 retry reason／所属 Step／attempt／消耗／触发证据；用已有 action、验证报告、产物变更和外部状态变化构成可解释进展信号。先观测再调阈值，不让单一启发式自动宣判失败。
- **验收：** 同参数但外部状态持续推进不误杀；换参数反复同错能识别；429／网络断流／溢出／截断／验证失败不共用同一种重试；主请求、压缩、复核和子任务成本能对账到 Run。
- **兼容：** 不删除 provider adapter 的专有错误信息，不把任何一家供应商的恢复参数写死为跨模型通用规则。

### F11 · P1：定时循环在宿主接入与同任务连续性上不一致

- **Evidence／状态：`C`。** Desktop setup 启动 60 秒 Cron tick；CLI main 查到服务注册但未见 tick 驱动。Cron 每次新 session、无人值守审批拒绝；其回调不收集 live 事件，不代表 Kernel 不持久化。[DA-DESKTOP-CRON] [DA-CLI-CRON] [DA-CRON]
- **Finding／影响：** “工具能创建计划”不等于当前宿主会执行；新 session 定时任务也不等于原会话 Goal 的续跑。重复 fire 是否允许重叠、失败如何呈现需要产品契约，不能仅靠 fire-and-forget。
- **Path／建议：** 明确宿主调度 owner／存活条件，暴露可观测的启用状态；现有 Cron 继续负责时间触发，RunCoordinator 负责接纳。对定时新任务与同目标续跑分别定义关联、重叠、错过触发和通知策略，保留无人值守权限默认收紧。
- **验收：** Desktop／CLI 分别验证创建→到期→接纳→结果；owner 退出／双宿主／重复 tick／任务超时／审批等待均不失控；不依赖 GUI 私有回调才能执行共享能力。
- **兼容：** 不把已有每 fire 新 session 的行为静默改成复用旧会话；不因为要“无人值守”就自动批准高风险工具。

## 8. 冗余、复杂度与不该删的部分

| 对象 | 判断 | 为什么／怎么处理 |
| --- | --- | --- |
| `LoopPhase::ORDER` 与真实循环 | **确认语义漂移；非运行链重复** | 无生产消费，却声明固定顺序。先修文档／标用途；若无外部契约再考虑弃用，不能单纯为用上它重写循环。[DA-PHASE] |
| Complete／CompleteMessage 两条收尾分支 | **确认决策重复且已分叉** | 验证、Hook、保存、结束重复，adversarial 接入不等。按 F06 统一完成候选处理。[DA-COMPLETE] |
| 编辑后验证／完成计划／事实门禁／panel／Stop | **职责部分互补，决策分散** | 快速语法反馈、构建、文件存在、目标覆盖、用户策略不是同一件事。统一证据和最终决策，不把所有检查删成一次 LLM judge。[DA-DECORATOR] [DA-PLAN] [DA-PANEL] [DA-STOP] |
| 各类 retry、stall、提醒 | **存在收敛空间，不宜粗暴合并算法** | 错误种类不同，应统一预算／原因记录与停止权，避免每发现一种失败再堆一个提醒。[DA-MODEL] [DA-STALL] |
| Session EventStore＋RunStore/run_events | **不是同义冗余** | 分别记录会话语义与运行执行证据。应检查投影与提交一致性，而非因为两种存储就合表。[DA-RUNSTORE] [DA-READ] |
| InputLease＋RunCoordinator／持久 dispatch | **不是重复互斥锁** | 局部排队／运行取消和跨连接执行所有权不同。应让新入口复用，不删除其中一层然后绕过接纳。[DA-COORDINATOR] |
| Response items＋Message | **必要兼容投影；双写要受控** | 目前 items 是权威；Message 仍供展示／兼容。不能继续分别修改形成两个模型历史真源，也不应直接删除 UI 投影。[DA-MODEL] [DA-COMPLETE] |
| Runtime event＋harness DTO＋Tauri DTO | **必要的边界分离** | UI 视图与机器协议不同；收敛映射和语义测试，不共用一个不断膨胀的万能 DTO。[DA-HARNESS] |
| run_events＋runtime log | **用途不同，需控制重叠** | 可回放业务事实与脱敏诊断日志不是同一物；不要两边都存完整 prompt／token。保留关联 ID、聚合与脱敏策略。[DA-KERNEL] [DA-LOG] |
| 现有 workflow＋未来 Goal | **互补，不应产生双运行中心** | DAG／WorkflowAgent 是执行结构，Goal 是目标控制。Goal 只接纳下一 Run，不能新造工具执行和终态系统。[DA-WORKFLOW] |
| 投机工具路径 | **条件能力，不是生产默认加速承诺** | 只有 safe／concurrency-safe／权限等满足且无 hooks 时启用；主装配挂 hooks。保留／去除应按真实收益与安全测试决定，不为追平参考产品直接放宽门禁。[DA-SPECULATION] [DA-ASSEMBLY] |

**收敛目标：一个完成裁决边界、一套接纳／权限／取消／终态事实、若干职责清晰的策略。不是把所有模块揉成一个大函数，也不是再增加一个总控服务包住旧链路。**

## 9. 后续迭代：先修契约，再做能力升级

### 9.1 分批路线与退出条件

下表全是 `P`，尚未实施；不预估缺少工程量依据的工期。每批可以独立审查与回滚，前一批验收通过再扩大自动化。

| 批次 | 要解决的问题 | 主要落点 | 交付物／退出条件 | 依赖及不做的事 |
| --- | --- | --- | --- | --- |
| A · P0，控制与结果可信 | F01、F02、F03、F09 的确定性部分 | TS SDK、protocol projection、Kernel 完成摘要、相关消费者／CI | burst 反例通过；一 Run 一 start／terminal；验收失败不再被解释成通过；各入口同一解释；原有终态提交测试保留 | 先记录旧契约及 golden fixtures；不做 Goal／远程 daemon |
| B · P1，收敛完成与停止决策 | F05、F06、F10 | Runtime 完成分支、Reflection／Stop／预算记录、AppCore 验证装配 | 单一候选完成入口；所有续跑有可解释原因与上界；局部策略不重复发命令／Hook；证据摘要完整 | 依赖 A 的摘要／身份；不把所有 advisory 改成强制阻塞 |
| C · P1，平台控制闭环 | F04、F07、F11 | AppCore 接纳／RunCoordinator、app-server、Rust／TS SDK、Desktop 桥接 | 明确 replace 与真正 steer；有界 typed read／reconcile；宿主调度可用性明确；跨入口场景测试 | 复用既有 lease、dispatch、cancel、store；不新增第二 app core |
| D · P2，显式目标外环 | F08 | 现有 session／control 持久化及 AppCore 安全 idle 边界 | 目标可查／暂停／恢复／完成／耗尽；生命周期权限明确；预算与证据跨 Run 保留；无进展停止 | 依赖 A–C；数据和协议迁移须另行评审；不新增工具执行中心 |
| E · P2，持续效果优化 | F09 的行为评测、F10 的成本调参 | 现有 run_events／指标的脱敏导出、仓库 fixtures／CI／评测脚本 | 固定任务基线与对照实验；失败归因可追溯；改动带回归样例及成本／质量差分 | 基础评测可与 A 并行准备；主动事件／多代理扩容只有收益证据后再接入 |

### 9.2 推荐下一轮最小任务包

优先只选 **A1：修复 SDK early-event 竞态并常态化该测试**。原因是已稳定复现、边界小、无需真实模型／沙箱，不依赖新数据库或运行时设计。

随后选 **A2：定义完成摘要与事件粒度的契约测试**，先不改默认策略。写清：执行结束是什么、必需验收通过是什么、best-effort 是什么、一个 Run 有多少次 start／terminal、哪些增量只是 provisional。拿到 golden fixture 后，再做 B 的完成分支收敛。

真正 steer 和 Goal 都不应与第一次完成策略重构捆成一个大 PR；否则接纳、取消、持久化与验收同时变化，失败很难归因。本文没有创建这些开发任务、定时任务或后台监听。

### 9.3 跨层变更的强制验收矩阵

| 场景 | 必须证明的约束 | 建议测试层 |
| --- | --- | --- |
| start 回复前／后同批事件、并发 Thread | 原始 delta／terminal 不丢失，不错分给其他运行 | TS／Rust SDK transport fixture、stdio 集成 |
| 两轮工具、多次 verification／Stop 回流 | 协议 Turn 身份不变，Step／attempt 可区分，最终只有一次终态 | runtime＋protocol golden＋app-server |
| 必需检查失败、advisory 失败、未运行检查 | 三者不同；耗尽停止不等于验收通过，历史回放一致 | runtime／AppCore／UI 与 JSONL 消费者 |
| tools／approval／verify 中用户介入 | steer／replace 语义明确；旧输入不双消耗；权限快照不被新消息放宽 | admission／RunCoordinator／SDK 集成 |
| 取消／deadline／Hook 失败／持久化失败 | canonical terminal 在 commit 后仅一次；不能先发成功再改失败 | 现有 terminal commit／故障注入测试扩展 |
| 断连前有副作用，结果未确认 | 不自动重发；可查询 action／terminal／approval，未知事实有显式状态 | RunStore reopen／SDK reconcile |
| 长上下文、压缩后 resume、截断／fallback | 关键约束和工具配对保留，旧 attempt 残片不污染新历史 | provider adapter／ModelAgent fixture |
| 显式 SandboxBackend 不可用或越界 | 不能静默降级 Direct；验证、MCP／命令按各自权限边界审计 | backend 单测＋真实 Windows 集成 |
| 定时／Goal 与用户输入竞争 | user／approval／cleanup 优先；不双发，不默认扩大权限 | 假时钟调度＋接纳集成 |
| 多 provider 与 reasoning／usage | 供应商增量被正确规范化；辅助调用不漏记／重记成本 | models adapter＋usage 回归 |

## 10. 如何持续优化：Loop engineering 的工程闭环

### 10.1 每一步的定义、现状与补齐方式

这一节是**改进 harness 的开发闭环**，不是每次用户请求要自动跑的八阶段生产流水线。

| 步骤／定义 | DeepAgent 当前可复用资产 | Codex／Claude 公开参照 | 建议输出与验收 |
| --- | --- | --- | --- |
| E1 定义目标：什么结果、约束和证据才算成功 | CompletionPolicy、VerificationPlan、workflow 目标 | Codex Goal；Claude Goal／验证 skills | 任务条件、必需／可选检查、预算和禁止事项；不能只有“让 agent 更聪明” |
| E2 建立基线：固定输入、版本、权限、环境与度量 | scripted Agent、mock transport、已有审计 fixture | Codex 本地 core／app-server tests；Claude 提取样本不能作为完整测试资产 | 相同任务 fixture、模型／adapter／配置版本和环境说明；先跑旧版 |
| E3 采集行为：记录动作、反馈、消耗和退出原因 | RuntimeEvent、run_events、action／approval、metrics | 两者 loop 都有可追踪的工具往返；官方 SDK／harness 文档 | 脱敏轨迹关联 session／Run／Step／attempt／call；不采集私密思维或完整凭据 |
| E4 归因：找第一个错误来源而非只看最终答复 | 现有失败注入、持久终态和 SDK burst 夹具 | OpenAI improvement-loop 的反馈／诊断方法 | 分类为接纳、上下文、provider、工具、权限、验证、事件、恢复、模型判断；一个可验证假设 |
| E5 固化反例：把失败变成可重复测试／eval | 大量现有 audit tests，但部分默认不跑 | 公开工程方法强调真实反馈进入可持久回归 | 确定性 bug 用单测／集成；行为问题用任务与 grader；禁止只写一段提醒代替回归 |
| E6 小步改策略：只改变一个可归因因素 | 既有 provider seam、pipeline、Kernel、SDK | OpenAI harness engineering 强调环境可读与机械约束；Anthropic 长任务示例强调增量产物／进度 | 一项修复／配置差异，附不变量与回滚办法；不复制第二套链路 |
| E7 同条件验证：比较质量、成本、稳定性和安全 | CI、Rust／TS 测试、Desktop 构建与手动工具 | 公开方法可借鉴，但双方内部发布门禁本轮不可见 | 旧／新版本差分，反例变绿且原有门禁不回退；真实模型结果多次重复，不只跑一次 |
| E8 发布与反馈：经审查推广并持续观察 | Git、版本化 DTO、现有日志／运行账本 | 官方 improvement-loop 可加入人工 review／发布门 | 变更说明、兼容迁移、指标变化；新失败再回 E3，不自动自改生产策略 |

参照范围：[CX-CORE-TESTS] [CX-SERVER-TESTS] [OA-HARNESS-ENG] [OA-IMPROVEMENT] [AN-LONG-HARNESS]；这不是两家内部团队开发流程的还原。DeepAgent 的主要工程缺项是把上述资产串成可反复运行、有发布约束的闭环，而非没有测试或没有日志。

### 10.2 首批评测不应只统计最终文本

| 指标 | 定义／用途 | 当前基线 |
| --- | --- | --- |
| 证据支持的任务成功率 | 外部 grader／真实测试满足目标的任务数 ÷ 有效任务数；不能用“模型说完成”代替 | 未建立同任务三方 benchmark，本文不填数值 |
| 假成功率 | 对外表示验收成功，但必需证据失败／缺失的任务占比 | F02 已证明存在相应设计路径，未测真实发生率 |
| 控制正确性 | start／steer／replace／cancel／approval 竞态的身份与副作用约束通过率 | 本轮只执行文末指定的有限测试 |
| 流与恢复正确性 | 原始事件保存率、terminal 唯一性、replay／重连一致性 | SDK burst 已有失败反例；不能由 default 7 passed 推断此项通过 |
| 每个已验证成功任务的成本 | 主模型＋压缩＋复核＋子代理等总消耗／已验证成功数 | 有记账与 metrics 资产，尚无本次对照测量 |
| 无进展消耗 | 没有新证据／新外部状态／有效产物变化期间的 token、请求和时间 | 先设计进展标签，再校准，不凭相同参数次数直接定性 |
| 延迟与资源 | 首事件、完整执行、工具等待、审批等待、持久化／重连耗时，分布而非均值 | 未执行性能实验；不能承诺收敛分支必然更快 |
| 安全不变量 | 未批准副作用、跨 workspace 越界、静默沙箱降级、敏感信息外泄 | 需要专门安全／OS 验收，不由模型成功率覆盖 |

同模型／同 adapter 的本系统改前改后比较，用于隔离 harness 改动；三方不同模型的整体产品比较另行标注，不能把模型变化归功于 loop。保留独立的 held-out 任务，避免只优化到现有反例；基线、阈值与样本量要在实际评测任务里确定，不在本报告凭空编分数。

## 11. 本轮验证、复现与交付边界

### 11.1 实际执行的行为检查

日期为 2026-11-01；以下均为本轮执行，不是复用前序统计。测试使用仓库已有代码／夹具，未调用真实模型，不修改业务实现。

| 命令／位置 | 结果 | 能证明什么 |
| --- | --- | --- |
| `pnpm test`，`packages/sdk` | 构建成功，7 passed | 当前默认 mock SDK 测试通过；不包含所有 audit 文件 |
| `$env:RUN_HARNESS_AUDIT='1'; node --test tests/early-events-audit.test.mjs`，`packages/sdk` | 1 passed / 2 failed，退出码 1 | F01 两种同批事件顺序均丢失原始高层 stream 事件；反例已复现，尚未修复 |
| `cargo test -p deepagent-runtime --offline --lib verification_`，仓库根 | 3 passed | 通过、失败后修复、重复失败停止修复三条既有语义；第三条当前允许完成 |
| `cargo test -p deepagent-runtime --offline --lib terminal_commit_tests`，仓库根 | 5 passed | canonical 终态提交／投影失败、Hook 失败、deadline 等既有故障场景 |
| `cargo test -p deepagent-harness-protocol --offline`，仓库根 | 12 contract tests passed；lib／doc-tests 各 0 | 请求与事件 shape／cursor 等契约通过；不代表 F03 粒度问题消失 |

F01 的最小复现（前提：本地已有依赖；本轮未安装或升级依赖）：

```powershell
Set-Location -LiteralPath 'G:\Code_Warehouse\DeepAgent-Studio\packages\sdk'
pnpm build
$env:RUN_HARNESS_AUDIT = '1'
node --test tests/early-events-audit.test.mjs
Remove-Item Env:RUN_HARNESS_AUDIT
```

预期观察：controlled 接收原始 delta 与 terminal；event-first／response-first 的 raw 已收到原始两事件，而高层 stream 在 release 前为空。夹具路径见 [DA-SDK-AUDIT]，额外 cleanup terminal 只为让失败测试正常结束，不能当修复。

### 11.2 未执行与不确定项

- 本轮没有跑完整 workspace、全量 clippy／fmt、Desktop build／GUI、真实 provider／MCP／Windows 隔离联调；因为仅新增研究文档，未改 Rust／TS 行为。后续实施必须按第 9 节做相应检查。
- 没有编译／运行 Codex 参考仓库，也没有运行 Claude 提取样本；对它们的实现结论来自静态阅读与明确的官方契约，不冒充动态产品验证。
- 没有同任务成功率、成本或性能对照实验；优势仅限已证实的架构资产和平台位置。
- 前序调研曾记录 AppCore 广测有一项构建 fixture 时限失败、单独复跑通过，根因未定位；本轮没有将它重新验证为稳定通过，不宣称全仓全绿。
- 当前公开文档可比本地参考样本更新；引用 `/goal`、Stop cap、SDK 行为时均保留来源层级。后续实施前再次固定版本并核对契约。

### 11.3 文档检查与本轮变更

仅新增本文。已执行 Markdown 结构与引用检查：66 个证据引用定义，其中 51 个本地路径／目录、15 个官方网页；本地路径、源码行锚点范围、引用解析、表格列数、标题层级、代码围栏及占位内容检查通过。暂存 diff 与 `git diff --cached --check` 检查通过，暂存区只含本文；不提交本地运行目录、锁文件、临时搜索文件或构建产物。本文不改变 API、配置、权限、沙箱或数据库。

## 12. 可定位的证据索引

### 12.1 DeepAgent 源码与本地夹具

以下引用均相对本文所在目录，绑定第 1 节源码基线；`#L` 为一基行号。行锚点定位到关键起点，相关分支通常在其后。它们证明实现结构，不自动证明生产环境效果。

| 证据组 | 定位内容 |
| --- | --- |
| 主链与边界 | [DA-ASSEMBLY]、[DA-WORKFLOW]、[DA-HARNESS]、[DA-COORDINATOR] |
| 内循环与状态 | [DA-LOOP]、[DA-MODEL]、[DA-PHASE]、[DA-KERNEL]、[DA-CONFIG] |
| 工具与验证 | [DA-PIPELINE]、[DA-VERIFY-CMD]、[DA-VERIFY]、[DA-REFLECT]、[DA-PLAN]、[DA-DECORATOR]、[DA-SANDBOX] |
| 完成、停止与复核 | [DA-COMPLETE]、[DA-STOP]、[DA-PANEL]、[DA-STALL]、[DA-SPECULATION] |
| 运行事实与协议 | [DA-RUNSTORE]、[DA-RECOVERY]、[DA-PROJECTION]、[DA-REQUESTS]、[DA-READ]、[DA-STEER] |
| SDK 与调度 | [DA-SDK-STREAM]、[DA-SDK-AUDIT]、[DA-SDK-PACKAGE]、[DA-SDK-README]、[DA-CRON]、[DA-DESKTOP-CRON]、[DA-CLI-CRON] |
| 工程检查 | [DA-CI]、[DA-METRICS]、[DA-LOG]、[DA-TERMINAL-TEST]、[DA-VERIFY-TEST] |

### 12.2 Codex 与 Claude 参考实现

| 系统 | 证据及边界 |
| --- | --- |
| Codex | [CX-TURN]、[CX-SAMPLING]、[CX-PARALLEL]、[CX-ORCHESTRATOR]、[CX-STEER]、[CX-REPLAY]、[CX-GOAL-SOURCE]；测试资产：[CX-CORE-TESTS]、[CX-SERVER-TESTS] |
| Claude 2.1.88 提取样本 | [CC-QUERY]、[CC-TOOLS]、[CC-EXECUTOR]；提取样本不等于官方完整源码，也不用于证明当前版本 cap／Goal 的内部实现 |

### 12.3 本轮打开的一手公开资料

| 资料 | 在本文中的用途 |
| --- | --- |
| [Codex app-server][CX-APP-SERVER] | 当前用户 Turn／steer／事件外部契约 |
| [Using Goals in Codex][CX-GOALS] | 目标外环与完成／预算边界 |
| [How Claude Code works][CC-WORKS] | loop 与 harness 概念边界 |
| [Claude hooks][CC-HOOKS] | 当前 Stop cap 与 StopFailure 语义 |
| [Claude sessions][CC-SESSIONS] | 会话恢复与未完成工具处理 |
| [Claude sandbox][CC-SANDBOX] | shell sandbox 范围及原生 Windows 限制 |
| [Claude Agent SDK overview][CC-SDK]、[Agent loop][CC-SDK-LOOP] | SDK 与 CLI 共核；turn／result／budget 定义 |
| [Claude Goal][CC-GOAL]、[Scheduled tasks][CC-SCHEDULE] | 目标与定时循环的不同控制契约 |
| [Loop engineering][CC-LOOP-ENG]、[Dynamic workflows][CC-DYNAMIC] | 触发／停止分类及任务编排模式，不作为本地旧样本证明 |
| [OpenAI harness engineering][OA-HARNESS-ENG] | 环境可读、机械约束与反馈工程方法，不当作 Codex 默认固定流水线 |
| [Agent improvement loop][OA-IMPROVEMENT] | traces → 反馈 → eval → harness 修改的开发流程示例 |
| [Anthropic long-running harness][AN-LONG-HARNESS] | 增量产物／进度／验证的研究示例，不当作每次 Claude Code 的默认流程 |

[DA-ASSEMBLY]: ../crates/deepagent-app-core/src/run_assembler.rs#L1224
[DA-WORKFLOW]: ../crates/deepagent-app-core/src/run_assembler.rs#L1680
[DA-HARNESS]: ../crates/deepagent-harness-protocol/src/lib.rs#L1
[DA-COORDINATOR]: ../crates/deepagent-app-core/src/run_coordinator.rs#L164
[DA-LOOP]: ../crates/deepagent-runtime/src/loop_engine.rs#L697
[DA-MODEL]: ../crates/deepagent-runtime/src/model_agent.rs#L2343
[DA-PHASE]: ../crates/deepagent-runtime/src/phase.rs#L13
[DA-KERNEL]: ../crates/deepagent-runtime/src/kernel.rs#L27
[DA-CONFIG]: ../crates/deepagent-runtime/src/loop_engine.rs#L130
[DA-PIPELINE]: ../crates/deepagent-runtime/src/tool_pipeline.rs#L370
[DA-VERIFY-CMD]: ../crates/deepagent-app-core/src/verification_commands.rs#L1
[DA-VERIFY]: ../crates/deepagent-runtime/src/loop_engine.rs#L2493
[DA-REFLECT]: ../crates/deepagent-verification/src/reflection.rs#L77
[DA-PLAN]: ../crates/deepagent-app-core/src/completion_plan.rs#L1
[DA-DECORATOR]: ../crates/deepagent-app-core/src/verification_decorator.rs#L49
[DA-SANDBOX]: ../crates/deepagent-app-core/src/sandbox_backend.rs#L29
[DA-COMPLETE]: ../crates/deepagent-runtime/src/loop_engine.rs#L881
[DA-STOP]: ../crates/deepagent-runtime/src/loop_engine.rs#L2558
[DA-PANEL]: ../crates/deepagent-app-core/src/verification_panel.rs#L490
[DA-STALL]: ../crates/deepagent-runtime/src/stall_detector.rs#L1
[DA-SPECULATION]: ../crates/deepagent-runtime/src/loop_engine.rs#L384
[DA-RUNSTORE]: ../crates/deepagent-persistence/src/run_store.rs#L205
[DA-RECOVERY]: ../crates/deepagent-app-core/src/service.rs#L94
[DA-PROJECTION]: ../crates/deepagent-harness-protocol/src/events.rs#L230
[DA-REQUESTS]: ../crates/deepagent-harness-protocol/src/requests.rs#L1
[DA-READ]: ../apps/cli/src/app_server.rs#L541
[DA-STEER]: ../apps/cli/src/app_server.rs#L944
[DA-SDK-STREAM]: ../packages/sdk/src/index.ts#L728
[DA-SDK-AUDIT]: ../packages/sdk/tests/early-events-audit.test.mjs#L1
[DA-SDK-PACKAGE]: ../packages/sdk/package.json#L15
[DA-SDK-README]: ../packages/sdk/README.md#L1
[DA-CRON]: ../crates/deepagent-app-core/src/cron_service.rs#L30
[DA-DESKTOP-CRON]: ../apps/desktop/src-tauri/src/lib.rs#L7211
[DA-CLI-CRON]: ../apps/cli/src/main.rs#L308
[DA-CI]: ../.github/workflows/ci.yml#L1
[DA-METRICS]: ../crates/deepagent-tracing/src/metrics.rs#L1
[DA-LOG]: ../crates/deepagent-app-core/src/runtime_event_log.rs#L62
[DA-TERMINAL-TEST]: ../crates/deepagent-runtime/src/kernel/terminal_commit_tests.rs#L1
[DA-VERIFY-TEST]: ../crates/deepagent-runtime/src/loop_engine.rs#L4641
[CX-TURN]: ../借鉴/codex/codex-rs/core/src/session/turn.rs#L163
[CX-SAMPLING]: ../借鉴/codex/codex-rs/core/src/session/turn.rs#L1629
[CX-PARALLEL]: ../借鉴/codex/codex-rs/core/src/tools/parallel.rs#L44
[CX-ORCHESTRATOR]: ../借鉴/codex/codex-rs/core/src/tools/orchestrator.rs#L205
[CX-STEER]: ../借鉴/codex/codex-rs/core/src/codex_thread.rs#L554
[CX-REPLAY]: ../借鉴/codex/codex-rs/core/src/session/rollout_reconstruction.rs#L9
[CX-GOAL-SOURCE]: ../借鉴/codex/codex-rs/ext/goal/src/runtime.rs#L425
[CX-CORE-TESTS]: ../借鉴/codex/codex-rs/core/tests/suite/
[CX-SERVER-TESTS]: ../借鉴/codex/codex-rs/app-server/tests/suite/v2/
[CC-QUERY]: ../借鉴/claudecode/restored-src/src/query.ts#L241
[CC-TOOLS]: ../借鉴/claudecode/restored-src/src/services/tools/toolExecution.ts#L337
[CC-EXECUTOR]: ../借鉴/claudecode/restored-src/src/services/tools/StreamingToolExecutor.ts#L265
[CX-APP-SERVER]: https://learn.chatgpt.com/docs/app-server
[CX-GOALS]: https://developers.openai.com/cookbook/examples/codex/using_goals_in_codex
[CC-WORKS]: https://code.claude.com/docs/en/how-claude-code-works
[CC-HOOKS]: https://code.claude.com/docs/en/hooks
[CC-SESSIONS]: https://code.claude.com/docs/en/sessions
[CC-SANDBOX]: https://code.claude.com/docs/en/sandboxing
[CC-SDK]: https://code.claude.com/docs/en/agent-sdk/overview
[CC-SDK-LOOP]: https://code.claude.com/docs/en/agent-sdk/agent-loop
[CC-GOAL]: https://code.claude.com/docs/en/goal
[CC-SCHEDULE]: https://code.claude.com/docs/en/scheduled-tasks
[CC-LOOP-ENG]: https://claude.com/resources/articles/getting-started-with-loops
[CC-DYNAMIC]: https://claude.dev/blog/a-harness-for-every-task-dynamic-workflows-in-claude-code/
[OA-HARNESS-ENG]: https://openai.com/index/harness-engineering/
[OA-IMPROVEMENT]: https://developers.openai.com/cookbook/examples/agents_sdk/agent_improvement_loop
[AN-LONG-HARNESS]: https://www.anthropic.com/research/effective-harnesses-for-long-running-agents
