# DeepAgent Studio 对照 Claude Code 的缺陷与修复（深度实码核查版）

> 生成日期：2026-09-29（第二版，重写）
> 参照系：`docs/claudecode-harness-research-report.md`；Claude Code 还原源码 v2.1.88（`借鉴/claudecode/restored-src/src`）
> 方法：本版逐链路重读本系统源码（33 crates / 约 16 万行 Rust），每条结论给出本系统 `文件:行号` 与 CC `文件:行号` **双方实据**。CC 侧还原源码本身不完整（sourcemap 缺部分文件），凡 CC 证据缺失处单独标注，不臆断。
> 边界（程序员Eighteen 已确认，沿用）：harness 模型固定 DeepSeek 官方 responses API 不走第三方；无限画布经 SDK 接入、与 harness 不相干 → **模型供应商接入不出现在修复项**。核心主线：① harness 完善；② 系统提示词减重；③ tools/skills/mcp/rules 渐进加载、避免上下文暴涨。

---

## 0. 本版与前版的差异：误判回收

前一份《缺陷及修复文档》基于浅扫，多处结论与实码不符。本版逐条修正：

| 前版结论 | 深挖后实状 | 裁决 |
|---|---|---|
| P0-3「循环判据缺 `needsFollowUp`」 | [model_agent.rs:1983-2014](crates/deepagent-runtime/src/model_agent.rs#L1983-L2014)：`tool_invocations_from_items()` 非空 → `CallTool/CallTools`，为空 → `CompleteItems`。与 CC `needsFollowUp`（[query.ts:558/834/1062](借鉴/claudecode/restored-src/src/query.ts#L1062)）**语义等价**：CC 流式看到 tool_use block 置 true，本系统看最终响应 items 里有无 tool invocation。 | ❌ 误判，**已实现** |
| P3-11「deepagent-prompts 孤岛（双源）」 | `AgentDef`/`frontmatter`/`load_command_file`/`discover_commands` 均被生产消费（subagent_runner.rs:974/1046、plugin_security.rs:236/291、slash_runtime.rs:1099）。**只有** `SystemPromptBuilder` 本尊（builder.rs:35）无生产消费方，其生产组装在 `system_context.rs` 完成。 | ⚠️ 半误判，改为「拼接层双实现」，见 2.5 |
| P0-1「assemble() 无预算=上下文暴涨」 | +`ContextPolicy` 已完整（policy.rs:45-123）：DeepSeek 1M 窗口 → 管理预算封顶 300k（policy.rs:25/135），自动压缩阈值复刻 CC 公式（effective−13k）。历史/tool 的裁剪由 model_agent 压缩链负责；`assemble(usize::MAX)` 只管**系统提示词 manifest**（量小）。 | ⚠️ 半误判，真正的病灶是 MCP instructions（见 2.1），assemble 预算缺失是**无兜底** |

---

## 1. 对照矩阵总表

每个模块三态：✅ 已实现（含实据）/ 🟡 实现但有真实缺口 / 🔴 完全缺失。

| 维度 | 本系统实据 | CC 实据 | 判定 |
|---|---|---|---|
| 静态/动态 prompt 分区 + 缓存边界 | `SYSTEM_PROMPT_DYNAMIC_BOUNDARY`（system_context.rs:14） | prompts.ts:573 boundary | ✅ |
| output style 注入 | output_style_prompt_block（system_context.rs:277） | prompts.ts:562 | ✅ |
| 项目结构上下文 | WorkspaceScanner 有界扫描（system_context.rs:82） | `directory` 高值上下文 | ✅ |
| 指令文件加载（CLAUDE.md/AGENTS.md/rules） | assembler.rs:173-223 双布局加载 | rules/CLAUDE.md 加载 | ✅ |
| 上下文预算策略 | `ContextPolicy`（policy.rs）+ DeepSeek 300k 封顶 | autoCompact.ts | ✅ |
| 自动压缩（阈值/溢出/时间触发） | model_agent.rs:1002/1829/637（含 prefire 两遍） | query.ts:629/1062 | ✅ |
| 历史/工具载荷裁剪 | `PromptBudget::fit`（budget.rs）—**但无生产消费方**，见 2.2 | prompt budget | 🟡 2.2 |
| 工具 deferred 渐进加载 | tool_manifest.rs 全链（默认 Auto @8k 字符，settings.rs:1705） | `deferred_tools_delta` | ✅ |
| 工具加载名单披露 | deferred_tools_announcement（tool_manifest.rs:93） | tools.ts | ✅ |
| skills 渐进披露 | `SkillCatalogSendState::next_delta`（skill_catalog_reminder.rs:85）+ 字符预算 | skills delta | ✅ |
| MCP instructions | **每轮全量重发**（`render_mcp_instructions`，mcp_runtime.rs:159-169） | **`mcpInstructionsDelta.ts` 增量 attachment** | 🔴 P0 见 2.1 |
| 记忆检索注入 | bm25/semantic/hybrid + `<system-reminder>`（model_agent.rs:1183-1216） | 4 层记忆 | ✅ |
| 记忆 compact 保留 | 记忆块通过 reminder/attachment 注入 | compact 保留记忆段 | ✅ |
| hooks 挂点全集 | 25+ HookPoint（deepagent-hooks） | hooks §4 | ✅ |
| hooks.json 外部钩子 JSON 协议 | external_hooks.rs：Exit code + stdout `{decision:block/allow}`（external_hooks.rs:51） | hooks JSON §4.4 | ✅ |
| 权限规则声明式（allow/ask/deny） | permission_rules.rs（冒号前缀匹配，对齐 CC `Bash(git:*)`） | settings permissions | ✅ |
| 审批统一路由 | approval_bridge.rs `PendingApprovals` + `ChannelApprovalGate`（call_id oneshot，可并发） | approval request DTO | ✅ |
| 子代理 runner | subagent_runner.rs（run/controlled/background/resume/审批桥） | subagents | ✅ |
| 子代理 frontmatter 字段 | `AgentDef` 仅 name/description/tools/model/color/body（agent_def.rs:44-58） | AgentDefinition（还原源码缺文件，无法逐字段证） | 🟡 2.4 |
| 任务体系（Task/v2） | 有 `Task` 概念（session 内 task state machine） | tasks/ 目录（LocalAgentTask/LocalMainSession/Remote/Dream） | 🟡 2.3 |
| Cron / scheduled tasks | **无**（grep ScheduledTask 全零命中） | useScheduledTasks.ts + 后台任务 | 🔴 2.3 |
| Teammate 寻址 | **无** | InProcessTeammateTask / RemoteAgentTask（tasks/） | 🔴 2.3 |
| 意图/输入分发层 | `deepagent-intent`（slash + 附件 + ExecutionRequest），**本系统独有** | CC 无独立 intent 层 | ✅ 亮点 |
| 规划引擎 | planner DAG（dag.rs 拓扑层）+ 3 策略，**本系统独有** | CC 无独立 planner crate | ✅ 亮点 |
| 观测/时间线/统计 | deepagent-observation（replayable timeline/stats）+ tracing metrics | telemetry §9.5 | ✅ 但无产品级遥测台 |
| 沙箱（微软/Sandboxie/无） | SandboxMode 三态 + sandbox_instructions（permissions_prompt.rs） | sandbox | ✅ |
| 迁移框架 | 无 migrations 基建 | migrations/ | 🔴 2.6 |

---

## 2. 真实缺陷清单（本轮深挖确认）

### 2.1【P0】MCP instructions 每轮全量重发，无增量披露

- **本系统实据**：`render_mcp_instructions`([mcp_runtime.rs:159-169](crates/deepagent-app-core/src/mcp_runtime.rs#L159-L169)) 遍历**每个已连接 server**，将其 `instructions` 完整文本拼成一个 `McpCatalog` block → 经 [system_context.rs:130-139](crates/deepagent-app-core/src/system_context.rs#L130-L139) 进动态段（每轮重发）。**无分页、无增量、无缓存**。`instructions` 是 server 实际返回的文本，非本系统控制；MCP server 越多 / 越啰嗦，每轮 prompt 动态段越大。
- **CC 实据**：v2.1.88 已实现**两条路径**——旧全量 `getMcpInstructions`（prompts.ts:579-604，dynamic 段，与`本系统现状相同`）之上，新增 **`mcpInstructionsDelta`**（[mcpInstructionsDelta.ts](借鉴/claudecode/restored-src/src/utils/mcpInstructionsDelta.ts)）：按 server **name** diff，未宣布过的才追加，并以 **attachment 持久化**到历史（`mcp_instructions_delta` 类型消息），已宣布的 server 不再重复进上下文；断开连接的 server 用 `removedNames` 记录。闸门默认 growthbook/ant，但实现完整可被 env 开启。
- **为何是 P0**：这正是「重复加载导致上下文暴涨」的活样本。CC 已给出解法；本系统停留在旧路径。
- **修复方案**（对齐 CC delta）：
  1. 新增 `McpInstructionsState`（per-session），记录已宣布 server name 集。
  2. 每轮由 `getMcpInstructionsDelta` 等价逻辑算出 `(addedNames, addedBlocks, removedNames)`，只发新增 block。
  3. 参考 CC 用「attachment 持久化」承载：在本系统 run-event 流上落一个 `EventPayload::McpInstructionsDelta { added, removed }`，重放时从事件重建已宣布集合（replay-safe，与 §5.2 事件可 replay 铁律一致）。
  4. 保留全量为 fallback 开关（对齐 CC `CLAUDE_CODE_MCP_INSTR_DELTA` env 语义，本系统用 `DEEPAGENT_MCP_INSTR_DELTA`）。
- **验证**：单测（首连宣布 → 次轮 0 新增 → 断连被移除）；重建历史后增量判定幂等。

### 2.2【P1】`PromptBudget::fit` 无生产调用 + `assemble(usize::MAX)` 无系统提示词预算闸

- **本系统实据**：
  - `PromptBudget::fit`（budget.rs:57-108 逻辑完整，有裁剪测试）**grep 全 workspace 无生产消费方**——只有 `policy.prompt_budget()` 构造器（policy.rs:75）与 `builder.rs` 文档提及。一条完整、可用的 budget 裁减链路是**死代码**。
  - `build_system_manifest` 唯一生产点传 `usize::MAX`（system_context.rs:152），即系统提示词 manifest 不做预算裁剪。历史与工具确实有压缩/裁剪两道防线，但 **MCP instructions、skill 目录、项目结构、插件输出样式等系统提示词块没有第二道闸**。
- **组合效应**：2.1 的 MCP 全量 + 这里无兜底 = 在 Server 数多时动态段可无界膨胀且不触发任何告警（ContextPolicy 只对历史+工具算）。
- **修复方案**：
  1. 给 `build_system_manifest` 接真实预算：从 `ContextPolicy.prompt_budget` 传 `assembler.assemble(.., prompt_budget)`。
  2. 接线 `PromptBudget::fit`：让 `system_context.rs` 组装 manifest 后，对非 required 块再跑一次 fit（system/required 保底，其余按 source_rank+priority 裁），消除死代码。
  3. 预估 token 溢出时记录 `dropped_origins` 进日志（assembler.rs:43 已有该字段）。
- **验证**：单测「mcp/skill 块超预算被裁、系统块保留」；现有 system_context 测试保持绿灯。

### 2.3【P1】Cron / Teammate / 完整任务体系缺失

- **本系统实据**：`grep ScheduledTask/scheduled_task/TaskScheduler/teammate` 全 crates 零命中。有 `todo_write`/`task_list` 工具（浅层任务跟踪），无定时调度、无跨会话任务队列、无 Agent-to-Agent 寻址。
- **CC 实据**：
  - 任务体系：`tasks/` 目录含 LocalAgentTask、LocalMainSessionTask、LocalShellTask、RemoteAgentTask、DreamTask、InProcessTeammateTask（[tasks/](借鉴/claudecode/restored-src/src/tasks/)）。
  - Cron：`useScheduledTasks.ts`（scheduled_tasks.json 持久化 + 每 root 锁 + 打瞄）。
  - Teammate：InProcessTeammateTask / RemoteAgentTask（跨会话寻址）。
- **修复方案**（可延后，P1 尾部）：
  1. Cron：`~/.deepagent/scheduled-tasks.json` + JSON-RPC 面；纯 `tokio::time` 实现，全本地可测。
  2. Teammate：先建「异步执行上下文隔离」（`tokio::task_local!` / 显式 agent_id 推进事件），再上寻址——否则多 agent 事件串流会污染。
  3. 任务持久化：run 事件已由 session 承载，Task 只需在 session 事件上加任务类型 tag，避免第二套 store（遵守 AGENTS.md §5.1 不新增第二套 run store）。
- **验证**：cron 单元测试（时间推进）；teammate 用隔离事件流单测。

### 2.4【P1】子代理 frontmatter 字段少

- **本系统实据**：`AgentDef`（agent_def.rs:44-58）解析字段 `name/description/tools/model/color/body`。模型偏好只有 `ModelPref`（Inherit/具体模型，agent_def.rs:24）。
- **CC 实据**：CC 的 agent `.md` frontmatter 支持 permissionMode、当用场景、hook 挂点等扩展字段。**但**还原源码中定义 `AgentDefinition` 的文件缺失（sourcemap 不完整），无法逐字段列出；为遵守「不臆断」，此处只标注「字段能力少于 CC」而不断言具体全集。
- **修复方案**：按调研报告 §5 的 Agent 定义形状，给 `AgentDef` 增 `permission_base`、`when_to_use`、`max_turns`（可选）；`tools` 语义已有 allowlist（subagent_runner.rs:1216 `runtime_agent_tool_allowlist`）。
- **验证**：frontmatter 解析新字段单测 + 既有 agent 加载测试。

### 2.5【P1】提示词拼接双实现（`SystemPromptBuilder` vs `system_context`）

- **本系统实据**：`SystemPromptBuilder`（builder.rs:35-180）是一套完整分层拼接（core/safety/workspace/identity/tool_rules/memory/context/user_goal），**但无生产调用**；生产统一走 `system_context.rs` 的 `ContextManifest`。两套 `PromptSource`/`PromptFragment` 语义并存，未来接 AGENTS.md 人设/规则时容易漂移。
- **修复方案**：二选一收敛。建议保留 `system_context.rs`（已在生产、有 boundary 纪律），把 `builder.rs` 并入或标注 deprecated；AGENTS.md 规则注入统一走 assembler 的指令文件加载（已存在，assembler.rs:173-223）而非新链路。
- **验证**：删除/re-export 后 workspace 全量编译 + 测试。

### 2.6【P2】无迁移框架 / 无产品级遥测

- **迁移**：无 migrations 目录。CC 有 [migrations/](借鉴/claudecode/restored-src/src/migrations/)（版本史迁移）。修复：建轻量框架（schema 版本号 + 幂等清单 + 启动执行），不为 persistence 现有 lexicon 做具体迁移，只备未来演进。
- **遥测**：有 `deepagent-tracing` + `deepagent-observation`（replayable timeline/stats），无 CC §9.5 的 GrowthBook/Datadog/OTel 1P 双 sink。修复：不引入云上报（DeepSeek 合规边界），在 tracing 之上做 session 级本地事件台账即可。

---

## 3. 我（及前序调研）以为缺失、但实码已存在的清单

用户自评/前序文档曾标记为缺失或存疑、本轮深挖确认为「已实现不修」：

| 疑点 | 实码实据 |
|---|---|
| 「needsFollowUp 续轮判据」 | model_agent.rs:1983-2014，语义等价（见 §0） |
| 「deferred 工具机制」 | tool_manifest.rs 全链，默认 Auto @ 8k 字符 |
| 「skills 渐进披露」 | skill_catalog_reminder.rs:85 `next_delta` + 字符预算 + reset |
| 「hooks 挂点简单」 | 25+ HookPoint 全集 + registry/lifecycle/builtin/external |
| 「hooks 外部 JSON 协议」 | external_hooks.rs stdout `{decision: block/allow}` + exit code |
| 「auto-compact / 上下文压缩」 | 阈值压缩（model_agent.rs:1002）、溢出反应式（:1829）、时间 microcompact（:637）、prefire 两遍（:1130） |
| 「上下文预算策略」 | `ContextPolicy` 300k 管理窗口封顶 + CC 公式压缩阈值 |
| 「沙箱」 | SandboxMode 三态 + 权限提示词 |
| 「审批路由」 | approval_bridge.rs PendingApprovals（call_id oneshot，并发可） |
| 「记忆注入」 | 检索引擎 + `<system-reminder>` 注入 + 预取（model_agent.rs:1183） |
| 「意图层/slash/附件」 | deepagent-intent（本系统独有亮点） |
| 「规划引擎」 | planner DAG + 3 策略（本系统独有亮点） |

---

## 4. 修复路线图

### S1（P0）：上下文减重收口——对齐主线②③
1. **S1-1** MCP instructions 改为 delta 增量 + 事件持久化（2.1）。
2. **S1-2** `assemble` 接真实预算 + 接线 `PromptBudget::fit` 消除死代码（2.2）。
3. **S1-3** 增补「系统提示词块超预算被裁 / MCP delta 幂等」测试。

### S2（P1）：harness 能力补齐——对齐主线①
4. **S2-1** Cron + scheduled-tasks.json 持久化（2.3）。
5. **S2-2** Teammate 异步上下文隔离，再上寻址（2.3）。
6. **S2-3** 子代理 frontmatter 扩展字段（2.4）。
7. **S2-4** 提示词拼接单源收敛（2.5）。

### S3（P2）：基础设施
8. **S3-1** 迁移框架（2.6）。
9. **S3-2** session 级本地事件台账（遥测下限，不云上报）（2.6）。

---

## 5. 验证纪律

- 每修复项：`cargo fmt --all -- --check` + 相关 crate 测试 + 跨 crate 改 `cargo test --workspace`。
- 提示词/token 类改动必须附带 token 计数断言（`HeuristicTokenizer` 在 context crate 已可用）。
- 无法无头验证的行为（遥测、UI 呈现），提交信息如实标注。
- 涉及事件/持久化新增（2.1 的事件源）：补重放测试，遵守 AGENTS.md §5.2（事件可 replay、终态只落一次）。