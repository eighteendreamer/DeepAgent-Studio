# DeepAgent Studio 记忆系统与意图识别实现盘点 · 对照 `10-claude-code.md` 与 `5-agent.md`

> 生成时间：2026-10-05
> 对照对象：`docs/10-claude-code.md`（Claude Code 六层记忆体系 + 横向缓存）、`docs/5-agent.md`（Agent 意图识别与路由）
> 取证方式：本仓库实码逐文件核对（`crates/*` + `apps/desktop/src-tauri` + `apps/desktop/src`）。所有结论都带 `文件:行号`。文末「取证与勘误」说明哪些是本人实读实跑、哪些来自子代理并已二次核验。
> 路径缩写：`$AC = crates/deepagent-app-core/src`、`$R = crates/deepagent-runtime/src`、`$M = crates/deepagent-memory/src`、`$CT = crates/deepagent-context/src`、`$B = crates/deepagent-builtins/src`。

---

## 0. 结论速览

### 0.1 记忆体系（对照六层）

| 层 | 文档定义的核心问题 | 本系统状态 | 进度 |
| --- | --- | --- | --- |
| ① 指令记忆 | 应该怎么做 | 双文件（`CLAUDE.md`+`AGENTS.md`）× 两级作用域（User/Project）+ 嵌套目录动态发现 + `InstructionsLoaded` 事件；**缺 Managed 组织级层** | 70% |
| ② 短期会话记忆 | 刚才聊了什么 | 事件流为唯一真值 + `recover`/`fork`/`rewind` + 从事件折叠历史并保工具配对；`ContextPolicy` 按真实模型预算算窗口 | 95% |
| ③ 工作任务记忆 | 现在做到哪 | `TaskState` 六态状态机 + 事件持久化 + `todo_write` 结构化待办 + `completion_plan` 完成证据；**任务态不跨会话续存** | 65% |
| ④ 长期记忆 | 以前记住什么 | **两套并存且互不相通**：chunk-level（knowledge 磁盘 md）全链路已接线；item-level（`MemoryItem`/`MemoryTier`）只有 `memory_write`/`memory_recall` 两个工具；文档点名的 Fork 子 Agent 写记忆、读写互斥**未实现** | 45% |
| ⑤ 摘要记忆 | 太长了怎么压 | 三层压缩（工具结果裁剪 / micro-compact / 完整 summary）+ 断路器 + `ContextCompacted` 事件；阈值与 Claude Code 同源（13K buffer / 20K summary reserve）；**缺"后台异步渐近摘要"**，压缩后不重注入项目规则与任务目标 | 60% |
| ⑥ AutoDream | 乱了怎么整理 | **基本没有**：无离线去重/合并/重写/重建索引，无 TTL/过期，item-level 每次新 id | 5% |
| 横向缓存 | 怎么少重复干活 | Prompt Cache 命中在模型侧、本系统只负责稳定 Prefix 编排 + cache token 记账；本地缓存有失效判定（mtime/hash） | 60% |

### 0.2 意图识别（对照 `5-agent.md` 生产方案）

| 组件 | 文档要求 | 本系统状态 | 进度 |
| --- | --- | --- | --- |
| Intent + Slot 结构化 | 把自然语言转成 `{intent, slots, tasks}` | **没有**。`InputIngress::normalize` 只分 `Prompt / SlashCommand / Shell` 三类；`deepagent-intent` 的 `IntentRouter`+`ExecutionRequest` 全仓零消费者 | 10% |
| 规则→小模型→LLM 分级 | 逐级升级 | 没有分级流水线；**"决定做什么"完全由模型在可见工具集上自选** | 5% |
| State 多轮更新 | Intent 随对话更新 | 有会话态（`DiscoveredToolSet`、plan mode、invoked skills）但**不含意图/槽位** | 25% |
| Router 入口分流 | Chat/RAG/SQL/Tool/Agent/Planner | 部分：slash 注册表、技能目录、工具懒加载、MCP 增量公告、权限风险分级都在起作用，但没有集中 Router 层 | 40% |
| Planner + Task Graph | 多意图拆解与并行编排 | 已接线：`HeuristicPlanner`（**仅启发式模板，无 LLM planner**）→ `PlanDag` → `DagScheduler` → `plan_execute` 工具 | 55% |
| Aggregator | 多路结果汇总 | 部分：DAG 有终态聚合，子代理结果回主循环；无独立汇总阶段 | 40% |
| Policy | 模型决策 ≠ 执行权限 | **强项**：`RiskLevel` + `PermissionSet` + `SafetyClassifier` + `PolicyGate`/`ChannelApprovalGate` + LLM 命令守卫 + 审批持久化 | 85% |
| Fallback / 澄清 | 低置信度→澄清，失败→升级 | 澄清已真接通（`ask_user_question` 弹框挂起）；stall nudge 有；**失败升级矩阵未实现** | 45% |
| 评估 | 错误路由率/澄清率/OOD/任务成功率 | 只有 `golden_trace.rs` 执行链快照，**无意图准确率类指标**；意图判定不落事件，无法回放"为什么走 A 路径" | 10% |

### 0.3 一句话结论

**记忆体系的"骨架"（会话真值、上下文压缩、指令加载、检索注入）已经成型且大多有测试；意图识别则是明显的短板——本系统走的是"把决定权整个交给模型"的 Claude Code 式路线，`5-agent.md` 里那套 Intent/Slot/State/Router 结构化前置层基本不存在，且 `deepagent-intent` 里已经写好的路由器和执行请求对象处于零调用状态。**

---

## 1. 本系统记忆体系解析（按六层）

### 1.1 ① 指令记忆｜规则级

**已实现**

- 根清单加载：`$CT/assembler.rs:173 load_instruction_files`，候选为 `~/.claude/CLAUDE.md`、`~/.deepagent/AGENTS.md`（`:176-177`）与 `<workspace>/CLAUDE.md`、`<workspace>/AGENTS.md`（`:193-194`）；同目录内 `AGENTS.md` 后列胜出（`:199`）。渲染带 token 预算：`assembler.rs:99 assemble(counter, token_budget)`，包封为 `<workspace-instructions>`（`:246`），由 `build_run_context`（`$AC/context_runtime.rs:956`）注入。
- 嵌套动态发现 + 反转加载：`$AC/nested_instructions.rs:56 NestedInstructionsDecorator`（`ToolResultDecorator`），`:99 candidate_dirs` 沿父目录向上收集后 `:134 dirs.reverse()`，即文档 1.2 的"先向上遍历收集，再从全局到局部反向加载"。限额：单文件 24KB（`:48`）、单次调用最多 3 文件（`:51`）、去重集（`:60`）。接线于 `$AC/run_assembler.rs:888-890`。
- 双轨注入（文档 1.4）：通道 A = 指令文件进上下文；通道 B = `$AC/system_prompt.rs` 的系统行为规范。**两条轨都在，且分离正确。**
- 留痕：`InstructionsLoaded` 事件在根加载（`run_assembler.rs:775`）与嵌套发现（`nested_instructions.rs:218`）两处发出。

**差距**

- 只有 User / Project 两级，**没有 Managed（组织级统一策略）**，也没有 Local（项目内私有、不入库）级的显式分层与优先级链。文档的 `Managed → User → Project → Local` 四级在这里是 2/4。
- workflow 与子代理分支拿不到嵌套发现装饰器：`run_assembler.rs:1180` 处 `nested_instructions: None`。
- 测试：`nested_instructions.rs` 4 个、`assembler.rs` 2 个、`run_assembler.rs` 3 个。

### 1.2 ② 短期会话记忆｜Session 级

**已实现（本层最完整）**

- 事件流是唯一真值：`$P/event_store.rs:155 append`、`:334 load_session`、`:340 read_from`、`:913 fork_session`、`:954 truncate_after`。权威事件在 `<data>/files/sessions/<前缀>/session.g*.jsonl.zst`，SQLite `events` 表只是索引。
- 会话对象：`deepagent-session/src/lib.rs:185 recover`、`:211 fork`、`:235 rewind`、`:273 append_response_item`。
- 历史从事件折叠，并**保工具调用配对**：`$AC/input_runtime.rs:257 conversation_with_tool_pairs_from_events`，`:284 flush_batch` / `:304 close_orphans` 为孤儿 `tool_use` 合成配对结果，`:444 bounded_tool_result` 控尺寸。
- 窗口与预算：`$CT/policy.rs:29 ContextPolicy{context_window, prompt_budget, auto_compact_at, warning_at, danger_at}`，`:45 for_capability` 按真实模型能力算比例，不是写死数字（文档 2.1 的 "Context Window + Token Budget 约束"）。
- 断线/崩溃恢复：`$AC/service.rs:72 recover_unfinished_runs`。
- 测试：`event_store.rs` 24、`input_runtime.rs` 9（含 snip 重放、配对重放、孤儿、超限）、session 10、`tests/golden_trace.rs`、`kernel_v2_e2e.rs`。

**差距**：这一层没有实质缺口。文档说的"截断/压缩/摘要/召回长期记忆"四件事，前三件在 ⑤，第四件在 ④，都已存在。

### 1.3 ③ 工作任务记忆｜Task 级

**已实现**

- 状态机：`deepagent-core/src/task.rs:19 TaskState{Queued, Running, Paused, WaitingApproval, Failed, ...}`；`create_task`（`run_assembler.rs:679,1210`、`prompt_gate.rs:49`）、`transition_task`（`run_assembler.rs:722/737`、`service.rs:108`）；持久化为 `EventPayload::TaskCreated`/`TaskStateChanged`（`core/src/event.rs:72/:80`）。
- 结构化待办：`$B/todo_tool.rs:70 TodoStore` + `todo_write`，并有"每次更新后置一次性提醒位"（`:64-66`），对应文档 3.1 的"待办事项/下一步计划"。
- 完成证据与阶段目标：`$AC/completion_plan.rs`（含 `:820` 自纠错）+ `$AC/verification_decorator.rs:55`。

**差距**

- 文档 3.2 强调"任务完成即清理/销毁，只有有长期价值的才沉淀"。本系统任务态随会话事件持久化，但**没有"跨会话续跑同一个任务"的工作态恢复**（`recover_unfinished_runs` 恢复的是 run，不是任务清单语义）。
- "中间结论/失败尝试"目前只散在事件流里，没有文档 3.1 那种结构化 Task State 容器。

### 1.4 ④ 长期记忆｜Persistent 级（本层问题最大：三套并存）

**链路一：chunk-level（生产主链路，已接线）**

- 真值是磁盘 md：`deepagent-knowledge/src/vault.rs:46 scan` / `:84 write`，位置 `<project>/.deepagent/knowledge/` + 全局（`$AC/knowledge_service.rs:146`）；索引是派生物，reload 重建（`knowledge/base.rs:126/:148`）。
- 写入时机三种：`knowledge_write` 工具、run 结束自动捕获（`$AC/run_finalizer.rs:157 spawn_auto_capture` → `knowledge_service.rs:334 capture_from_session`，先落草稿，`accept_draft:295` 人工确认）、会话摘要（`:540`）。
- 检索：`base.rs:13 ContextualRetriever` + `HeadingContextualizer` + `ScoreReranker` + `cosine_similarity`，`HashingEmbedder`（**离线哈希，不是真 embedding**）。
- 注入：被动附件走 `AttachmentLayer::UserInput`（`$AC/context_runtime.rs:1128-1158`）。
- 桌面端有完整命令面板：`src-tauri/src/lib.rs:1771-1847`（list/search/get/save/delete/reload/passive/drafts/auto-capture）。
- 预算：`knowledge_service.rs:689 MIN_SCORE=0.35`、`:691 LIMIT=3`。

**链路二：item-level（D3，只到"工具级"）**

- `$AC/memory_service.rs:29 MemoryService` 把 `MemoryItem` JSON 写进 `DocumentStore` 的 `memory_items` 集合（`:26/:74/:86`，落 SQLite `documents` 表），按 importance×recency×frequency 排序（`$M/ranking.rs`）。
- 接线只有一条：`run_assembler.rs:448-459` 注入 `MemoryBackend` → `tool_runtime.rs:532 register_memory_tools`（该调用在 `build_main_run_toolset` 内，**子代理与 workflow 拿不到**）→ `$B/memory_tools.rs:119 MemoryRecallTool` / `:187 MemoryWriteTool`。
- 也就是说：**没有自动注入、没有自动写入、没有 Tauri 命令、没有 UI、没有治理**。它更像"给模型的两个 API"，不是记忆系统。
- 测试：`memory_service.rs` 3、`memory_tools.rs` 5、`ranking.rs` 4；**无跨会话端到端 run 测试**。

**链路三：写了但没人用的检索机件（已核实零消费者）**

`$M` 里的 `store::MemoryStore`、`semantic::SemanticMemoryStore`、`hybrid::{HybridConfig, HybridRetriever, to_l5_block, SignalReranker}`、`bm25::Bm25Index`、`fusion::reciprocal_rank_fusion`、`chunking`、`contextualize` 在 memory crate 之外**全部零引用**（逐符号 grep 确认，仅 crate 内自测）。`MemoryTier` 有 14 处引用（item-level 在用），但混合检索/语义存储整条路没接。

**对照文档的三条硬要求**

| 文档条目 | 本系统 |
| --- | --- |
| 4.1 memdir 分类（User / Feedback / Project / Reference + `MEMORY.md` 索引） | 无对应结构。knowledge 是扁平条目 + 标题去重；item-level 有 `MemoryTier` 五层但没有分类学 |
| 4.2 索引 + 动态召回，只召回当前任务需要的 | **已实现**：`relevant_memories` 后台预取（`$R/model_agent.rs:1180 maybe_schedule_memory_prefetch` / `:1213 collect_finished_memory_prefetch` / `:1253` 注入"# 相关记忆"），provider 由 `run_assembler.rs:853-856 with_relevant_memory_provider` 注入，实现 `knowledge_service.rs:708` |
| 4.3 AI 决策式召回（不靠关键词/向量相似度） | **未实现**。召回是 BM25/哈希向量 + 阈值 `MIN_SCORE`，没有让模型判断"这条记忆对本任务是否有用"的环节 |
| 4.4 Fork 子 Agent 写 Memory | **未实现**。自动捕获在主 run 的 finalizer 里同步/后台跑，但复用主模型客户端，没有独立记忆代理 |
| 4.5 读写互斥 / 版本 / CAS | **部分**。knowledge 靠文件 + reload 重建，item-level 靠 SQLite 单写；没有版本或 CAS，两个 run 同时写同一主题会互相覆盖 |

### 1.5 ⑤ 摘要记忆｜Compression 级

**已实现**

- 阈值与 Claude Code 同源且注释直接标了出处：`$CT/policy.rs:13 AUTOCOMPACT_SUMMARY_RESERVE_TOKENS = 20_000`（对齐 `autoCompact.ts::MAX_OUTPUT_TOKENS_FOR_SUMMARY`）、`:19 AUTOCOMPACT_BUFFER_TOKENS = 13_000`（对齐 `autoCompact.ts::AUTOCOMPACT_BUFFER_TOKENS`），`:108 autocompact_threshold_tokens` 由窗口减 buffer 得到——即文档 5.4 的"200K − 13K ≈ 187K"。可用设置或 `DEEPAGENT_AUTOCOMPACT_RESERVE_TOKENS` 覆盖。
- 三层压缩齐全：工具结果裁剪与 micro-compact（`$AC/context_runtime.rs:658 micro_compact_tool_results`，CLEARED_STUB `:664`，释放需 ≥20% 才采纳 `:231`；response-item 版 `:702`）、完整 summary（Stage2 `:259`，`CompactionPolicy{trigger 8000, keep 6}` 在 `$CT/compaction.rs:59-78`）、run 前历史压缩（`run_assembler.rs:1430 maybe_compact_history`）。
- 结构完整性：`context_runtime.rs:1218 pairing_safe_compaction_split` 保证不按 token 硬切破坏 tool_use/tool_result 配对（文档 5.9 的"消息边界/Tool Use/Tool Result 完整性"）。
- 防抖与断路器：`context_runtime.rs:44 CompactionBreaker`（COOLDOWN 15s、连续 3 次无效即停）、`$R/model_agent.rs:233 MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`、`:258 MICROCOMPACT_IDLE_SECS = 3600`。
- 可观测：`RuntimeEvent::ContextCompacted{strategy}`（`events.rs:324`），策略名如 `history_snip`（`model_agent.rs:1445`）、`reactive_model`（`:1857`），并持久化进 run_events（`runtime_event_log.rs:310-311`）。
- Hook：BeforeCompact / PostCompact（`context_runtime.rs:191/:238/:281`）。
- 测试密度最高的一层：`context_runtime.rs` 21、`model_agent.rs` 48、`compaction.rs` 6、`policy.rs` 5、`model_compactor.rs` 10。

**差距（与文档差异最大的一层）**

1. **没有"后台异步渐近摘要"。** 文档 5.1-5.3 的核心是双时间轴：主 Context 一直长，后台 Session Memory 独立滚动出 V1…V20，靠 `lastSummarizedMessageId` / `tokensAtLastExtraction` / `sessionMemoryInitialized` 记录"总结到哪了"。全仓检索这些状态与 session-memory 载体：**未找到**。本系统是"到阈值时同步压一次"，压缩发生在请求路径上，代价是该轮延迟。
2. **没有"Summary + Recent Raw ≤40K"的两段式重组。** 本系统保留策略是 `keep 6`（最近 6 条消息）+ 触发阈值，是**按消息条数**而非文档 5.6 的"最近原始消息 token 上限 40K"。
3. **压缩后重注入只有三类**：`context_runtime.rs:778 compaction_reinjection_block` 明确只收集 已改文件 / 失败工具与检查 / 已调用 skill（各上限 `MAX_ITEMS = 8`）。项目规则与任务目标**不重注入**（goal 只进 summary prompt，`:108`），这与 `AGENTS.md` §5.2 与内核重构计划第 3 条"压缩后重新注入项目规则、任务目标、修改文件、失败测试、已调用 Skills"仍有两项缺口。

### 1.6 ⑥ AutoDream｜离线级

**基本未实现。** 检索 `dedup|consolidat|reindex|merge_memor` 在 `$M` 与 `memory_service.rs` 内零命中；`retention|ttl|expire|forget` 同样零命中（`event_store.rs:539 purge_trashed_sessions_before` 是会话回收，不是记忆治理）。

现状能算"半个"的只有：knowledge 侧的标题归一与同题去重（`base.rs:81/:266/:168`），以及 item-level 的 `importance`（`$M/lib.rs:86`，写入默认 0.5，`memory_service.rs:67`）+ `access_count/last_accessed` 参与排序。

**缺**：离线扫描重复/冲突/过期 → 合并重写 → 重建索引的整条闭环；以及文档 6.4 点名的并发安全（文件锁 / 版本号 / CAS）。

### 1.7 横向缓存体系

| 文档三层 | 本系统 |
| --- | --- |
| 7.1 Harness 本地缓存 + 失效判定 | 有，且是显式的：codegraph / project map 懒建索引（`tool_runtime.rs:201 CodeIndexAutoBuild`）、vision 缓存、附件 sha256 校验；失效靠 mtime/hash。文档 7.6"缓存 ≠ 一致性"这条在本系统是成立的 |
| 7.2 Context / Session 复用 | 有：会话事件流 + `Session::recover` + `response_items_from_events`（`input_runtime.rs:363`）复用已解析的 provider item，不从零重建 |
| 7.3 Model / API Prompt Cache | **归属正确**：命中判断在模型/API 侧，本系统只做两件事——把稳定 Prefix 放前面（`context_runtime.rs` 的装配顺序 + MCP instructions delta 只在变化时重发），以及把 cache 命中 token 结构化记账（`UsageRecorded.prompt_cache_hit_tokens/miss`，前端 `TurnFooter`/`SessionUsageBar` 显示命中率）。符合文档 7.4"Harness 组织 Context，Model 判断命中" |

---

## 2. 本系统意图识别解析

### 2.1 真实调用链（一句话 → 决定做什么）

```
start_chat_run                      src-tauri/src/lib.rs:1905
  → ChatService::run_in_session_with_attachments   $AC/chat_service.rs:1116
  → RunAssembler::with_attachments().run()          $AC/run_assembler.rs:172
  → InputIngress::normalize()                        $R/input.rs:62   ← 唯一的"分类"，只分 Prompt/SlashCommand/Shell
  → maybe_handle_slash_command()                     run_assembler.rs:1285 → slash_runtime.rs
  → accept_input_turn()                              input_runtime.rs:34（:73 抢 InputLease）
  → select_run_model() :319 / plan_mode_for_session() :333 / collect_discovered_tools_from_events() :336
  → 工具注册                                         tool_runtime.rs:530
  → build_run_context()                              :747
  → create_task / transition_task(Running)           :679 / :722
  → ModelAgent::new :842 → AgentKernel::start        :915/:942
```

**关键事实：内核不做意图识别。**"决定做什么"由模型在当轮可见工具集上自选，本系统的职责是**收窄可见集 + 施加权限与风险约束**。这与 `5-agent.md` 主张的"先把 Intent+Slot 结构化再路由"是两种范式，不是同一范式做到不同完成度——对比时必须按这个前提读。

### 2.2 逐维状态

| # | 维度 | 状态 | 证据 |
| --- | --- | --- | --- |
| 1 | 入口分流 | 部分已接线 | `InputIngress::normalize`（`$R/input.rs:59-111`）只产 `InputMode{Prompt,Shell,Meta}` × `InputKind{Prompt,SlashCommand,ShellCommand}`；粘贴 marker 回填 `:75-81`；附件→消息 `run_assembler.rs:82`（reference-only）。**`@` 文件引用不在内核**：前端自己拼 `<context-items>`（`apps/desktop/src/App.tsx:271`），`deepagent-intent` 的 `extract_mentions` 只处理 `#path` 且未接线 |
| 2 | 意图层本体 | **存在但零调用** | `crates/deepagent-intent`：`Intent{Chat,SlashCommand,UnknownCommand}`（`request.rs:16`）、`ExecutionRequest`（`:51`，含 `allowed_tools:66`）、`IntentRouter`（`router.rs:24/:59`）。逐符号 grep：`IntentRouter`、`ExecutionRequest`、`Intent::Chat` 在本 crate 之外**零命中**；`ExecutionRequest.allowed_tools` / `restricts_tools()` 无任何调用点。已接线的只有 `slash.rs` 的注册表（`slash_runtime.rs:19`、`commands.rs:96`、`deepagent-prompts/src/command_loader.rs:12`）。crate 内 30 个单测覆盖的是**未接线代码** |
| 3 | 禁止关键词猜意图 | 生产路径基本干净，有 1 处残留 | 关键词打分的 `skills/registry.rs:172-207 match_query` + `:247 auto_activate` **不在 run 链路**，唯一入口是人工预览命令（`skills_service.rs:263` → `lib.rs:1267`）。仍在生产路径的字符串匹配：`chat_service.rs:1949`（模型名 + `/model` 的错误提示分支）、`run_assembler.rs:1408-1412`（硬编码 skill id 集 `{docx,xlsx,pptx,pdf}` → `OfficeSkillGuardHook`）。`classifier.rs:34 args_contains` 属权限分类，不是意图 |
| 4 | 计划模式与任务分解 | 已接线 | `plan_mode.rs:28 PlanMode` + `:58 PLAN_SAFE_TOOLS` + `:77 is_plan_safe_tool`，`PlanModeHook` 在 BeforeToolUse 拒非只读；接线 `hook_assembly.rs:71`、`kernel_runtime.rs:78`；`/plan`、`/execute` 见 `intent/slash.rs:16-20,261,266` → `slash_runtime.rs:106/110`；提醒装饰器 `plan_mode_reminder.rs:45`。Planner：`deepagent-planner/src/planner.rs:42 HeuristicPlanner`（**只有启发式模板，无 LLM planner**）→ `dag.rs PlanDag` → `subagents/scheduler.rs:54 DagScheduler` → `$B/plan_execute.rs:22` 工具 → `tool_runtime.rs:401/:531`。测试：`dag_orchestration` 4、`scheduler` 5 |
| 5 | 工具选择与路由 | 已接线 | `tool_manifest.rs:17 prepare_tool_manifest`、`:39 should_activate_tool_search`、`:93 deferred_tools_announcement`、`:122 build_visible_tool_schemas`；`$B/tool_search.rs:37/:86/:359`；会话态 `DiscoveredToolSet`（`chat_service.rs:724`、`run_assembler.rs:1330`）+ 事件回放（`:336`）+ 持久化 `EventPayload::ToolsDiscovered`（`core/event.rs:187`）；MCP `mcp_service.rs:924 list_tools` + instructions delta（`run_assembler.rs:996-998`）。**`tool_manifest.rs` 自身 0 单测**（同名逻辑测试寄居在 `chat_service.rs:2831-3010`） |
| 6 | 技能激活 | 已接线（模型自决） | 两级加载 `skills/skill.rs:4-10`（L1 名+描述常驻、L2 正文按需、L3 资源不加载）；常驻目录 `$AC/context_runtime.rs:1086 build_skill_catalog_blocks` + `skill_catalog_reminder.rs:46`（send-once + delta，10 测试，预算紧张仍保 description `:127`）；显式调用 `$B/skill_tool.rs:37` ← `chat_service.rs:872`，Tauri `activate_skill`（`lib.rs:1271/6803`）；激活态提醒 `invoked_skills_reminder`（`context_runtime.rs:1462`，写入方 `hook_assembly.rs:303-326`） |
| 7 | 子代理分派 | 已接线，边界见勘误 | `$B/task_tool.rs` → `tool_runtime.rs:384/:530`；`ChatSubagentRunner`（`run_assembler.rs:407` + `subagent_runner.rs`）；agent 定义收集 `run_assembler.rs:393`，工具过滤 `chat_service.rs:59`；子代理提问走 `RecommendedResponder`（`run_assembler.rs:387`） |
| 8 | 失败与升级 | 部分 | 澄清已真接通：`$B/ask_user_tool.rs:22,56` → `$AC/question_bridge.rs:99 ChannelQuestionResponder`（4 测试）→ Tauri `respond_question`（`lib.rs:2468`，注册于 `:6871`），headless 走 decline/recommended，集成测试 `chat_service.rs:1409`。自纠错：`ModelAdversarialVerifier`（`run_assembler.rs:793-800`）、`verification_decorator.rs:55`、`completion_plan.rs:820`。Stall：`$R/stall_detector.rs`（`build_stall_nudge:330`）+ `ModelStallClassifier`，接线 `model_agent.rs:2051`、上限 `MAX_STALL_NUDGES_PER_RUN :2055`、事件 `StallNudgeInjected`。**失败升级矩阵未实现**（只有 `RunFailed`/`TaskState::Failed`/`fallback_model` 三条散点） |
| 9 | 可观测 | **严重不足** | `EventPayload`（`core/event.rs:57-203`）与 `RuntimeEvent`（`events.rs:44-402`）**都没有意图变体**，全仓 `IntentClassified` / `"intent"` 检索为空。唯一痕迹是 `run_requested` 日志里的 `input_kind: format!("{:?}", normalized_input.kind)`（`run_assembler.rs:218,232`）与 `input_lease_acquired`（`input_runtime.rs:80-88`）。`router.rs:70` 与 `:82` 的 `tracing::debug!("routed slash command")` 在**未接线代码**里。结论：能回放"做了什么"，**回答不了"这一轮为什么走 A 路径"** |

### 2.3 与 `5-agent.md` 的三个正面冲突

1. 文档主张"规则→小模型→LLM"三级前置识别；本系统只有一级（规则化的入口分流），其余全交给主模型。**这是范式差异**，不是完成度差异——但文档 §9"LLM 负责理解和提出决策，程序负责约束和执行"这一条本系统做得比文档更实（Policy 85%）。
2. 文档 §3/§8 要求"缺 Slot / 完全没说清 → Clarification"。本系统的澄清通道（`ask_user_question`）刚接通且**不超时、权限档不吞**，方向一致；缺的是"低置信度触发澄清"的判定入口——现在完全靠模型自己决定要不要问。
3. 文档 §11 要求评估错误路由率/澄清率/OOD。本系统只有 `tests/golden_trace.rs` 的执行链快照，没有意图层指标，也没有意图事件可统计。

---

## 3. 三处"存在但未接线"（本次盘点最有价值的产出）

| 位置 | 内容 | 影响 |
| --- | --- | --- |
| `crates/deepagent-intent` 的 `router.rs` / `request.rs` / `attachment.rs` | `IntentRouter`、`ExecutionRequest{allowed_tools}`、`extract_mentions` 全仓零消费者；crate 内 30 个单测测的是死代码 | 若照它继续加意图层，会形成"第二套路由"与实际生效路径（模型自选）不一致的假象；要么接线，要么删除 |
| `crates/deepagent-memory` 的 `store::MemoryStore`、`semantic::SemanticMemoryStore`、`hybrid::*`、`bm25::Bm25Index`、`fusion::reciprocal_rank_fusion`、`chunking`、`contextualize` | memory crate 之外零引用（逐符号 grep 确认） | 混合检索/语义存储能力"看着有"，实际生产只用 knowledge 的 ContextualRetriever + 哈希向量 |
| item-level 记忆（`$AC/memory_service.rs` + `$B/memory_tools.rs`） | 只挂了两个工具，无自动注入、无 UI、无自动写入、无治理、无跨会话端到端测试 | 与 chunk-level 形成两套互不相通的长期记忆；`MemoryTier` 五层分类学实际不产生召回价值 |

---

## 4. 建议优先级（仅结论，未实施）

| 优先级 | 事项 | 对应缺口 |
| --- | --- | --- |
| P0 | 压缩后重注入补上"项目规则 + 任务目标" | ⑤ 差距 3，且 `AGENTS.md` §5.2 与内核重构计划已明确要求 |
| P0 | 决定 `deepagent-intent` 的去留：接线成真实 Router，或删掉避免第二套路由 | §3 表第 1 行 |
| P1 | 意图判定落事件（`IntentClassified{kind, route, confidence}`）+ 进 run_events 白名单 | ② 维度 9，也是任何评估的前提 |
| P1 | 长期记忆收敛成一套：item-level 要么并入 knowledge 检索链，要么补自动注入 + 治理 | ④ 三套并存 |
| P2 | ⑤ 补"后台渐近摘要"：加 `last_summarized_seq` / `tokens_at_last_extraction` 状态，Compact 时复用最新摘要 + 最近原文按 token 上限（而非条数）保留 | ⑤ 差距 1、2 |
| P2 | ⑥ 最小版离线治理：一次/会话结束触发的去重+合并+重建索引，带版本号或 CAS 避免覆盖新写入 | ⑥ |
| P3 | ① 补 Managed 层；workflow/子代理分支接上 `nested_instructions` | ① 差距 |
| P3 | 记忆正文写入路径补脱敏（目前只有会话检索索引 redact，`event_store.rs:1161-1186`） | ⑦ |
| P3 | 意图评估集：从 golden trace 扩出错误路由率/澄清率指标 | 文档 §11 |

---

## 5. 取证与勘误

- 本文所有 `文件:行号` 均来自本次实码核对。其中以下条目由我本人直接读源码或跑命令确认：`nested_instructions.rs:99/:134` 的反转加载、`$CT/policy.rs:13/:19` 的 20K/13K 常量与出处注释、`context_runtime.rs:778` 重注入只含三类、`tool_runtime.rs:511/:530/:532` 证明 `task` 与 `memory_*` 只挂主运行注册表、`register_memory_tools` 唯一调用点、`HybridRetriever`/`SemanticMemoryStore`/`Bm25Index`/`store::MemoryStore`/`HybridConfig` 外部零引用（逐符号计数）、`IntentRouter`/`ExecutionRequest`/`Intent::Chat` 外部零命中、`ask_user_question` 全链（`ask_user_tool.rs` → `question_bridge.rs` → `respond_question` → `lib.rs:6871` 注册）。
- **勘误 1（已纠正）**：子代理报告称"子代理递归禁令未找到"。实为已实现——`register_task_tool` 只在 `build_main_run_toolset`（`tool_runtime.rs:511` 起，调用点 `:530`）内注册，子代理注册表由 `build_base_tool_registry` 构建（`run_assembler.rs:388`），不含 `task`；`$AC/chat_service.rs:757-760` 的注释也明确写了这条设计意图（对齐 Claude Code 的 agent-disallowed-tools 规则）。因此本文按"已实现"记录，缺的只是"没有显式深度上限常量"（因为天然只有一层）。
- **未验证项**：`relevant_memories` 的真模型端到端用例带 `#[ignore]`（`context_runtime.rs:2038`），本次未跑真模型；③⑥⑦ 的"未实现"结论来自全仓关键词检索为 0，属否定性证据，若后续新增实现需同步更新本文。
- 前端呈现核对：`apps/desktop/src` 与 `packages` 中检索 `relevant_memories|context_compacted|memory_write|memory_recall` 无命中，即这些记忆事件目前不在 UI 时间线呈现。
