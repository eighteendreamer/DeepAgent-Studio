# Implementation Plan

## Overview

本计划把设计落成可增量执行、每步可编译/可测试的编码任务。顺序遵循 design 的 7 个实施阶段：先纯后端核心 crate（离线可测），再工具，再 app-core 服务，再 ChatService 接线，最后 Tauri 命令与前端。每步保持 `cargo test --workspace --offline` 全绿、`fmt`/`clippy` 干净。

最大杠杆是**复用**现有基础设施：检索走 `deepagent-memory::ContextualRetriever`（chunk→contextualize→embed→BM25→RRF→rerank），frontmatter 解析走 `deepagent-skills::frontmatter`，离线嵌入走 `HashingEmbedder`，路径封闭性沿用 `WorkspaceRoot` 守卫。新增的只是 vault（markdown 真源）+ 捕获 + 检索注入编排。

## Tasks

- [x] 1. 搭建新 crate `deepagent-knowledge` 骨架并接入 workspace
  - 在 `crates/deepagent-knowledge/` 创建 `Cargo.toml`，依赖 `serde`、`serde_json`、`thiserror`、`tracing`、`deepagent-memory`、`deepagent-skills`、`deepagent-persistence`（按需），默认 offline，无网络 feature。
  - 在根 `Cargo.toml` 的 `[workspace] members` 加入新 crate。
  - 创建 `src/lib.rs` 暴露后续模块（`entry`、`vault`、`base`）的占位 `pub mod`，确保空骨架可 `cargo build --offline` 通过。
  - _Requirements: 7.1, 7.2, 8.1, 8.2, 8.4_

- [x] 2. 实现知识条目数据模型 `KnowledgeEntry` 与枚举
- [x] 2.1 实现 `EntryKind` 与 `Scope` 枚举
  - 在 `src/entry.rs` 实现 `EntryKind { Pitfall, Solution, Command, Config, Note }`，含 `label()`、容错 `parse()`（未知→Note）、`observation_type()` 映射到 `deepagent_memory::ObservationType`。
  - 实现 `Scope { Project, Global }`，`serde(rename_all = "snake_case")`。
  - 单元测试：`parse` 容错、未知 kind 落到 Note、label 往返。
  - _Requirements: 1.4_

- [x] 2.2 实现 `KnowledgeEntry` 及 markdown 往返
  - 实现 `KnowledgeEntry` 结构（id/title/kind/tags/created_at/updated_at/source_session/scope/body）。
  - 实现 `to_markdown()`（YAML frontmatter + body）与 `from_markdown(id, scope, raw)`（复用 `deepagent_skills::frontmatter` 解析；容错：缺 frontmatter→用文件名当 title、kind=Note）。
  - 实现 `searchable_text()`（title + tags + body）。
  - 单元测试：frontmatter 往返一致；容错解析（缺 frontmatter / 坏 kind）仍能载入正文。
  - _Requirements: 1.3, 1.4, 2.4, 2.5_

- [x] 3. 实现 `Vault`（markdown 真源磁盘 I/O）
  - 在 `src/vault.rs` 实现 `Vault { root, scope }` 与 `new/scope`。
  - 实现 `slug(title)`（小写、非字母数字转 `-`、压缩连续 `-`、截断、冲突追加短哈希）。
  - 实现 `scan()`：遍历 `*.md`、解析成条目、跳过坏文件并 `tracing::warn`（不阻断整库）。
  - 实现 `write(entry)`：路径限定 root 内（复用 `WorkspaceRoot` 同款穿越校验），越界返回错误。
  - 实现 `delete(id)`。
  - 单元测试：scan 多文件 + 跳过坏文件；write/delete 往返；`..` 穿越/越界绝对路径被拒（Property 2）；slug 冲突追加哈希。
  - _Requirements: 1.6, 2.1, 2.2, 2.3, 2.4_

- [x] 4. 实现 `KnowledgeBase` 门面（索引 + 检索 + 写入）
- [x] 4.1 定义 `KnowledgeConfig`、`KnowledgeHit`、`KnowledgeDraft`
  - 在 `src/base.rs` 实现 `KnowledgeConfig`，`Default` = `min_score 0.30 / max_inject 3 / max_inject_tokens 1200 / passive_enabled true`。
  - 实现 `KnowledgeHit { entry, score, excerpt }` 与 `KnowledgeDraft { title, body, kind, tags, source_session, scope }`。
  - _Requirements: 3.2, 3.6, 3.7, 6.3_

- [x] 4.2 实现索引构建与 `load_all`
  - 实现 `KnowledgeBase<E: Embedder>`，内部用 `ContextualRetriever`（chunk 级 + 上下文化）做检索，`entries: BTreeMap<id, KnowledgeEntry>`。
  - 实现 `new(vaults, embedder, config)` 与 `load_all()`：扫描所有 vault、重建索引（doc_id == entry.id）、返回载入条目数。
  - 单元测试：load_all 计数正确；多 vault 合并。
  - _Requirements: 6.2, 7.2, 7.4_

- [x] 4.3 实现 `search` 与阈值/预算过滤
  - 实现 `search(query, kind?, limit)`：走 `ContextualRetriever`，命中映射成 `KnowledgeHit`（excerpt 取命中 chunk，score 用归一化 rerank 分），可选 kind 过滤；命中时 `MemoryItem::touch` 更新访问统计。
  - 实现 `passive_block(query)`：search → 过滤 `score >= min_score` → 取前 `max_inject` 条 → 按 `max_inject_tokens` 截断 → 渲染带来源标注（`[source: 标题 (scope)]`）的上下文块；无命中返回空串。
  - 单元测试：命中相关条目；BM25 精确命中命令名/配置键/报错原文（Property 6）；低分被阈值过滤（Property 3）；条数与 token 预算上界（Property 4）；空 query/空库/无命中返回安全空（Property 7）。
  - _Requirements: 3.1, 3.2, 3.3, 3.6, 4.2, 4.4, 6.1, 6.2, 6.4, 6.5_

- [x] 4.4 实现 `write`/`list`/`get`/`delete`/`config`
  - 实现 `write(draft)`：slug 生成 id；若同 id 或归一化标题相等 → 更新（覆盖 body、刷新 updated_at、保留 created_at），否则新建；落盘后同步更新索引。
  - 实现 `list`/`get`/`delete`（delete 同步移除索引与磁盘文件）、`config`/`set_config`。
  - 单元测试：写入即可被检索（Req 1.7）；去重幂等——同 title 连写两次只一份文件、created_at 不变、updated_at 刷新（Property 8）；delete 后磁盘与索引一致（Property 1）。
  - _Requirements: 1.1, 1.2, 1.5, 1.7, 5.4, 5.5_

- [x] 5. 实现知识工具 `knowledge_search` / `knowledge_write`
  - 在 `crates/deepagent-builtins/src/knowledge_tools.rs` 定义工具名常量、`KnowledgeBackend` async trait（search/write）、`KnowledgeToolHit`/`KnowledgeToolDraft`。
  - 实现 `KnowledgeSearchTool<B>`：schema `{ query, kind?, limit? }`，`RiskLevel::Safe`、只读并发安全、无需审批；无命中返回成功空结果 `{ hits: [], count: 0 }`。
  - 实现 `KnowledgeWriteTool<B>`：schema `{ title, content, kind?, tags? }`，`RiskLevel::Low`、需要 `WorkspaceWrite`；返回 `{ id }`，失败走 `ToolOutput::failure`。
  - 在 `deepagent-builtins` lib 导出新模块。
  - 单元测试（用 stub backend）：search 命中/空结果、kind 过滤、write 返回 id、schema 校验、Safe/Low 风险等级与权限标注正确。
  - _Requirements: 4.1, 4.2, 4.4, 4.6_

- [x] 6. 实现 app-core `KnowledgeService`
  - 在 `crates/deepagent-app-core/src/knowledge_service.rs` 实现 `KnowledgeDto`/`KnowledgeHitDto`/`KnowledgeDraftDto`。
  - 实现 `KnowledgeService { inner: Mutex<KnowledgeBase<HashingEmbedder>> }`，方法 `open(project_root, global_root)`（双 vault 载入）、`reload`、`list`、`get`、`search`、`save`（新建/更新）、`delete`、`passive_block`、`set_passive_enabled`。
  - 在 app-core lib 导出，模式对齐 `SkillsService`。
  - 单元测试：open 合并双 vault；save 新建/更新；delete；search DTO 映射；passive_block 受 passive_enabled 控制。
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5, 7.2, 7.4_

- [x] 7. ChatService 接线（被动注入 + 工具注册 + 系统提示补强）
- [x] 7.1 实现 `KnowledgeBackend` 桥接 + `with_knowledge` builder
  - 在 app-core 实现把 `Arc<KnowledgeService>` 适配成 `deepagent-builtins::KnowledgeBackend` 的桥。
  - 给 `ChatService` 加 `with_knowledge(Arc<KnowledgeService>)` builder（对齐 `with_mcp`/`with_projects`）；未接时字段为 `None`。
  - _Requirements: 4.5_

- [x] 7.2 注册工具并补强系统提示
  - 在 `run_in_session` 组装主 registry 时：注册 `knowledge_search` + `knowledge_write`（真实 backend）；子代理 registry（`ChatSubagentRunner`）只注册 `knowledge_search`，不给 `knowledge_write`。
  - 在 `SYSTEM_PROMPT_BASE` 的 "Using your tools" 段补一句：遇不熟悉报错/重复性问题/项目约定先 `knowledge_search`；解决值得复用的问题后用 `knowledge_write` 沉淀。
  - _Requirements: 4.3, 4.5_

- [x] 7.3 实现被动注入
  - 在 `build_system_prompt` 之后、`SYSTEM_PROMPT_DYNAMIC_BOUNDARY` 之后追加 `knowledge_service.passive_block(prompt)` 结果（空串则不加）；受 `KnowledgeConfig.passive_enabled` 控制。
  - 单元测试：接 KnowledgeService 后被动注入块出现在 system prompt 动态段且在边界之后（Property 5）；命中条目被注入、无命中不注入；未 `with_knowledge` 时行为与引入前完全一致（Property 9 回归保护）。
  - _Requirements: 3.1, 3.4, 3.5, 3.7, 8.1_

- [x] 8. Tauri 命令 + AppState 接线
  - 在 `apps/desktop/src-tauri/src/lib.rs` 的 `AppState` 加 `knowledge: Arc<KnowledgeService>`（项目 vault = 活跃项目根 `.deepagent/knowledge/`，全局 vault = app 数据目录 `knowledge/`），并 `chat = ...with_knowledge(knowledge.clone())`。
  - 新增命令 `kb_list`、`kb_search`、`kb_get`、`kb_save`、`kb_delete`、`kb_reload`、`kb_set_passive`，薄封装 `KnowledgeService`，并在 `invoke_handler` 注册。
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5, 7.2_

- [x] 9. 前端知识库视图与接线
  - `apps/desktop/src/types.ts` 加 `KnowledgeEntry`/`KnowledgeHit` 类型；`api.ts` 加 `kbList/kbSearch/kbGet/kbSave/kbDelete/kbReload/kbSetPassive`（含 browser fallback）。
  - 新增 `apps/desktop/src/components/KnowledgeView.tsx`：列表（标题/kind/标签/更新时间）+ 搜索框 + 详情/编辑 + 删除；提示统一用既有 `message` 组件；保留既有 Tailwind 风格。
  - 挂到 Sidebar "知识库"入口（紧邻"技能"），补 i18n 文案（locales）。
  - 验证 `cd apps/desktop; pnpm build`（tsc+vite）通过，KnowledgeView 无诊断。
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5, 5.6, 8.3_

- [x] 10. 设置开关（被动注入 + 阈值/条数）
  - 在设置面板加被动注入开关（默认开启，调 `kb_set_passive`）；可选暴露 `min_score`/`max_inject` 调参。
  - 提示统一用 `message` 组件。
  - 验证前端 `pnpm build` 通过。
  - 实现说明：被动注入开关直接做在 KnowledgeView 头部（更易发现、不动设置面板样式），调 `kb_set_passive`，默认开启，提示用 `message` 组件。`min_score`/`max_inject` 为设计中标注的可选项，沿用 `KnowledgeConfig` 默认值。
  - _Requirements: 3.7, 6.3_

- [x] 11. 全量质量门禁回归
  - 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --offline -- -D warnings`、`cargo test --workspace --offline` 全绿。
  - 运行 `cd apps/desktop; pnpm build` 通过。
  - 修复任何回归，确保未 `with_knowledge` 路径的现有测试不变（Property 9）。
  - _Requirements: 8.1, 8.2, 8.3, 8.4, 8.5_

- [x] 12. 会话自动沉淀（方案 A：草稿 + 人工确认）
- [x] 12.1 条目状态 `EntryStatus` + 草稿 vault 隔离
  - `entry.rs` 加 `EntryStatus { Active, Draft }`，`KnowledgeEntry` 加 `status`（frontmatter `status`，缺省 `active` 兼容旧文件）；markdown 往返带上 status。
  - `vault.rs` 加 `scan_drafts`/`write_draft`/`delete_draft`（`.drafts/` 子目录，路径封闭、容错扫描同正式区）。
  - 单元测试：status 往返；草稿读写删；`.drafts` 与正式区互不串扫。
  - _Requirements: 9.5, 9.6_

- [x] 12.2 `KnowledgeBase` 草稿 API + 检索隔离
  - `load_all` 只把正式区进索引；草稿载入独立 `drafts` 映射，不进 retriever。
  - 实现 `add_draft`/`list_drafts`/`accept_draft`（删草稿+正式 write，透传 source_session）/`discard_draft`。
  - 单元测试：草稿不出现在 `search`/`passive_block`（Property 10）；accept 后正式区+1 草稿区-1、source_session 正确（Property 13）；discard 不影响正式库。
  - _Requirements: 9.5, 9.6, 9.8, 9.9, 9.12_

- [x] 12.3 错误恢复检测 `capture.rs`（纯函数）
  - 实现 `RecoverySignal` + `detect_recovery(&[Event])` + `is_worth_capturing`（至少一次工具失败 且 任务完成）。
  - `transcript_digest`：用户目标 + 失败工具/错误摘要 + 最终答案，按字符上限截断。
  - 单元测试：有失败+完成→worth；无失败→不 worth（Property 11）；空/无完成安全。
  - _Requirements: 9.1, 9.2_

- [x] 12.4 app-core `KnowledgeService::capture_from_session` + 草稿服务方法
  - 实现总结调用（非流式 `stream_chat`，要求模型只输出 JSON `{worth_saving,title,kind,tags,body}`）；`worth_saving=false`/解析失败/无 failure 一律返回 `None`。
  - 加 `list_drafts`/`accept_draft`/`discard_draft`/`set_auto_capture`/`auto_capture_enabled` DTO 包装。
  - 单元测试（MockTransport）：给定恢复事件 + mock JSON → 产出草稿 DTO；mock `worth_saving=false` → None；无 failure 事件 → None（不调用模型）。
  - _Requirements: 9.3, 9.4, 9.7, 9.10, 9.12_

- [x] 12.5 ChatService 接线（run 后异步 spawn，不阻塞）
  - `run_in_session` 成功返回后：若挂 knowledge 且 `auto_capture_enabled` 且能 build Chat 模型，则 `tokio::spawn` 后台调用 `capture_from_session`（读该 session 事件日志），**不 await**。
  - 无 key/失败静默跳过；未挂 knowledge/关开关行为不变。
  - 单元测试：错误恢复 run 后草稿区出现一条；平凡 run 后无草稿；关开关无草稿（Property 12）。
  - _Requirements: 9.10, 9.11, 9.1, 9.2_

- [x] 12.6 Tauri 命令 + AppState
  - 新增 `kb_list_drafts`、`kb_accept_draft`、`kb_discard_draft`、`kb_set_auto_capture`、`kb_auto_capture_enabled`，注册进 invoke_handler。
  - ChatService 后台 capture 用 AppState 的 rt/模型构造（与 run_chat 一致）。
  - _Requirements: 9.7, 9.8, 9.9, 9.10_

- [x] 12.7 前端草稿区 + 自动沉淀开关
  - `types.ts`/`api.ts` 加草稿类型与 `kbListDrafts/kbAcceptDraft/kbDiscardDraft/kbSetAutoCapture/kbAutoCaptureEnabled`（含 browser fallback）。
  - `KnowledgeView` 顶部加「待确认草稿」分区（醒目标识 + 采纳/丢弃按钮）；头部加「自动沉淀」开关（与「自动注入」并列）。提示用 message 组件，保留 Tailwind 风格，补 i18n。
  - 验证 `pnpm build` 通过。
  - _Requirements: 9.7, 9.8, 9.9_

- [x] 12.8 全量质量门禁回归（自动沉淀）
  - `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --offline -- -D warnings`、`cargo test --workspace --offline`、`pnpm build` 全绿。
  - 确认草稿隔离（Property 10）、触发保守（Property 11）、主 run 不受影响（Property 12）、采纳来源（Property 13）。
  - _Requirements: 8.1, 8.2, 8.3, 8.5, 9.5, 9.11_

## Task Dependency Graph

```text
1 (crate 骨架)
└─> 2.1 (EntryKind/Scope) ─> 2.2 (KnowledgeEntry md 往返)
        └─> 3 (Vault 磁盘 I/O)
              └─> 4.1 (Config/Hit/Draft)
                    └─> 4.2 (load_all 索引)
                          └─> 4.3 (search/passive_block)
                          └─> 4.4 (write/list/get/delete)
                                └─> 5 (knowledge 工具)
                                └─> 6 (KnowledgeService)
                                      └─> 7.1 (with_knowledge builder)
                                            └─> 7.2 (工具注册 + 提示)
                                            └─> 7.3 (被动注入)
                                                  └─> 8 (Tauri 命令)
                                                        └─> 9 (前端 KnowledgeView)
                                                              └─> 10 (设置开关)
                                                                    └─> 11 (全量门禁回归)
```

关键路径：1 → 2 → 3 → 4 → 6 → 7 → 8 → 9 → 11。任务 5（工具）与任务 6（服务）都依赖 4，可在 4 完成后并行。任务 10 依赖 9。任务 11 是最终汇合点。

```json
{
  "waves": [
    { "wave": 1, "tasks": ["1"], "rationale": "crate 骨架 + workspace 接入，所有后续任务的前置" },
    { "wave": 2, "tasks": ["2.1"], "rationale": "枚举模型，KnowledgeEntry 的前置" },
    { "wave": 3, "tasks": ["2.2"], "rationale": "KnowledgeEntry + markdown 往返" },
    { "wave": 4, "tasks": ["3"], "rationale": "Vault 磁盘 I/O，依赖条目模型" },
    { "wave": 5, "tasks": ["4.1"], "rationale": "Config/Hit/Draft 数据类型" },
    { "wave": 6, "tasks": ["4.2"], "rationale": "load_all 构建索引" },
    { "wave": 7, "tasks": ["4.3", "4.4"], "rationale": "检索与写入，均依赖索引，可并行" },
    { "wave": 8, "tasks": ["5", "6"], "rationale": "工具与服务都依赖 KnowledgeBase 完成，可并行" },
    { "wave": 9, "tasks": ["7.1"], "rationale": "with_knowledge builder + backend 桥，依赖服务" },
    { "wave": 10, "tasks": ["7.2", "7.3"], "rationale": "工具注册/提示补强与被动注入，可并行" },
    { "wave": 11, "tasks": ["8"], "rationale": "Tauri 命令 + AppState 接线" },
    { "wave": 12, "tasks": ["9"], "rationale": "前端 KnowledgeView + api/types" },
    { "wave": 13, "tasks": ["10"], "rationale": "设置开关，依赖前端" },
    { "wave": 14, "tasks": ["11"], "rationale": "全量质量门禁回归（基础特性汇合点）" },
    { "wave": 15, "tasks": ["12.1"], "rationale": "EntryStatus + 草稿 vault 隔离，自动沉淀的数据层前置" },
    { "wave": 16, "tasks": ["12.2", "12.3"], "rationale": "草稿 API/检索隔离 与 错误恢复检测，可并行" },
    { "wave": 17, "tasks": ["12.4"], "rationale": "总结调用 + 草稿服务方法，依赖 12.2/12.3" },
    { "wave": 18, "tasks": ["12.5"], "rationale": "ChatService 异步接线，依赖 12.4" },
    { "wave": 19, "tasks": ["12.6"], "rationale": "Tauri 命令 + AppState" },
    { "wave": 20, "tasks": ["12.7"], "rationale": "前端草稿区 + 自动沉淀开关" },
    { "wave": 21, "tasks": ["12.8"], "rationale": "自动沉淀全量门禁回归，最终汇合点" }
  ]
}
```

## Notes

- **范围控制**：embedding 磁盘缓存、相似度合并去重、用户级之外的更多 scope —— 均为后续增强，本期预留接口但不实现。会话「自动总结沉淀」已由 Task 12（方案 A：草稿 + 人工确认）落地。
- **向后兼容（Property 9）**：ChatService 未 `with_knowledge` 时行为与引入前完全一致，现有测试不得回归——这是每步验证的硬约束。
- **离线优先**：默认 `HashingEmbedder` + 本地 BM25，无网络依赖；任何网络能力走 feature gate。
- **真源唯一性（Property 1）**：磁盘 `.md` 是唯一真源，索引/embedding/缓存都是派生物；冲突时以磁盘为准。
- **环境注意**：Windows + PowerShell，长输出用 `> out.txt 2>&1` 重定向再读，避免管道命令被 `^C` 中断；桌面窗口运行时锁 exe，重编前先关窗口。
- 每个含代码的任务完成后即跑相关单元测试与 `fmt`/`clippy`，不要攒到最后。
```
